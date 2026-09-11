//! `invoke(tool_name, args)` — a first-class MCP tool, not a fallback, because Claude Code is
//! the primary client and `notifications/tools/list_changed` is unreliable: a client that
//! cached its tool list at session start can still reach any tool by name through `invoke`
//! without needing to notice the list changed. [`LIST_TOOLS_NAME`]'s companion tool exists for
//! the same reason — a callable way to re-check the tool set mid-session.
//!
//! [`dispatch`] is the one place `tools/call` (see [`super::handlers`]) hands off to actually
//! running something: it unwraps `invoke`'s `{tool_name, args}` wrapper (or `list_tools`'s empty
//! one) down to a plain `(name, args)` pair and a direct tool call is exactly that pair already,
//! so both shapes end up calling [`crate::runtime::Executor::run_tool`] through the same path.

use serde_json::{Value, json};

use crate::http::redact_message;
use crate::resolve::EndpointPlan;
use crate::runtime::{Executor, ExecutorError, RunResult, RunStatusView};
use crate::store::RunCallerKind;

use super::registry::{self, INVOKE_TOOL_NAME, LIST_TOOLS_NAME};

/// A tool call's result in MCP's `content`/`isError` shape, before JSON-RPC framing.
pub struct ToolCallOutcome {
    pub text: String,
    pub is_error: bool,
}

impl ToolCallOutcome {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Everything that keeps a `tools/call` from even reaching [`ToolCallOutcome`] — a protocol-level
/// problem (an unknown tool name, a malformed `invoke` wrapper) or a genuine internal failure
/// (the executor couldn't load auth providers or persist the run). Both are the caller's
/// [`super::handlers::tools_call`]'s job to turn into a JSON-RPC `error`, never a `200` with
/// `isError: true` — that envelope is for a tool that ran and didn't succeed, not for one that
/// was never identifiable in the first place.
pub enum DispatchError {
    UnknownTool(String),
    MissingToolName,
    Internal(String),
}

/// Resolves `name`/`arguments` (already unwrapped from the JSON-RPC `params` object) to a
/// [`ToolCallOutcome`], handling the two synthetic dispatcher tools before falling through to a
/// direct tool call.
pub async fn dispatch(
    executor: &Executor,
    plan: &EndpointPlan,
    name: &str,
    arguments: Value,
    caller_kind: RunCallerKind,
    caller_id: String,
) -> Result<ToolCallOutcome, DispatchError> {
    match name {
        LIST_TOOLS_NAME => Ok(ToolCallOutcome::ok(list_tools_text(plan))),
        INVOKE_TOOL_NAME => {
            let tool_name = arguments
                .get("tool_name")
                .and_then(Value::as_str)
                .ok_or(DispatchError::MissingToolName)?
                .to_owned();
            let inner_args = arguments.get("args").cloned().unwrap_or_else(|| json!({}));
            run_named_tool(
                executor,
                plan,
                &tool_name,
                inner_args,
                caller_kind,
                caller_id,
            )
            .await
        }
        other => run_named_tool(executor, plan, other, arguments, caller_kind, caller_id).await,
    }
}

async fn run_named_tool(
    executor: &Executor,
    plan: &EndpointPlan,
    name: &str,
    args: Value,
    caller_kind: RunCallerKind,
    caller_id: String,
) -> Result<ToolCallOutcome, DispatchError> {
    // `Executor::run_tool` already resolves `name` against the plan and returns
    // `ExecutorError::ToolNotFound` when it isn't there; checked again here so the "unknown tool
    // name" case (a caller typo, or a plan the client's cached tool list has drifted from) never
    // needs to build an `Executor`/`AuthProviders` first just to say so.
    if plan.tool(name).is_none() {
        return Err(DispatchError::UnknownTool(name.to_owned()));
    }
    match executor
        .run_tool(plan, name, args, caller_kind, caller_id)
        .await
    {
        Ok(result) => Ok(run_result_outcome(result)),
        Err(ExecutorError::ToolNotFound { name }) => Err(DispatchError::UnknownTool(name)),
        // A real, known tool that this build simply can't run yet (script execution is a later
        // chunk) is a fact about *this call*, not the protocol — `isError: true`, not a
        // JSON-RPC error.
        Err(ExecutorError::ScriptExecutionNotImplemented { name }) => Ok(ToolCallOutcome::error(
            format!("tool {name:?} is a script; script execution is not implemented in this build"),
        )),
        // Loading auth providers or persisting the run failed on our side, not the caller's —
        // that is a protocol-level problem (the server couldn't do its job), not "this call
        // didn't succeed".
        Err(e @ (ExecutorError::Auth(_) | ExecutorError::Record(_))) => {
            Err(DispatchError::Internal(redact_message(&e.to_string())))
        }
    }
}

/// A single direct/`invoke`d tool call always dispatches exactly one batch item (never a
/// script's own multi-call batch — that's C9), so [`RunResult::error`] is unambiguous: present
/// iff the run didn't reach [`RunStatusView::Ok`].
fn run_result_outcome(result: RunResult) -> ToolCallOutcome {
    let text = match &result.value {
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
        None => result.error.clone().unwrap_or_default(),
    };
    if result.status == RunStatusView::Ok {
        ToolCallOutcome::ok(text)
    } else {
        ToolCallOutcome::error(text)
    }
}

fn list_tools_text(plan: &EndpointPlan) -> String {
    serde_json::to_string(&registry::tool_list(plan)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_outcome_is_not_an_error() {
        let outcome = ToolCallOutcome::ok("hi");
        assert!(!outcome.is_error);
        assert_eq!(outcome.text, "hi");
    }

    #[test]
    fn error_outcome_is_flagged() {
        let outcome = ToolCallOutcome::error("boom");
        assert!(outcome.is_error);
    }

    #[test]
    fn run_result_ok_status_is_not_an_error_outcome() {
        let result = RunResult {
            run_id: uuid::Uuid::nil(),
            status: RunStatusView::Ok,
            value: Some(json!({"a": 1})),
            error: None,
        };
        let outcome = run_result_outcome(result);
        assert!(!outcome.is_error);
        assert_eq!(outcome.text, r#"{"a":1}"#);
    }

    #[test]
    fn run_result_non_ok_status_is_an_error_outcome() {
        let result = RunResult {
            run_id: uuid::Uuid::nil(),
            status: RunStatusView::Error,
            value: None,
            error: Some("upstream failed".to_owned()),
        };
        let outcome = run_result_outcome(result);
        assert!(outcome.is_error);
        assert_eq!(outcome.text, "upstream failed");
    }
}
