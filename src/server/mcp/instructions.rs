//! The `initialize` response's generic `instructions` text — split out of [`super`] to keep
//! that file under the size cap. [`super::initialize_result`] appends the endpoint's own
//! `EndpointPlan::instructions` (a human-authored, per-endpoint addendum) after this.

pub(super) const INSTRUCTIONS: &str = "\
# api2mcp

This endpoint exposes a curated set of HTTP API calls as MCP tools — each one a narrow, \
deterministic capability rather than a generic \"make an HTTP request\" tool. Call `tools/list` \
to see exactly what's available here; the set is specific to this endpoint, not the whole \
service.

## Calling a tool

Call a listed tool directly by name, or through `invoke` — pass `{\"tool_name\": ..., \"args\": \
...}` and it dispatches to the same tool a direct call would. Prefer `invoke`/`list_tools` over \
relying on a `tools/list_changed` notification to learn about tool set changes mid-session, since \
not every client re-fetches on it.

## Failure shape

A tool call that fails (bad arguments, an upstream error, a budget limit) still returns a normal \
`200` result with `isError: true` and a text explanation — it is not a JSON-RPC-level error. A \
JSON-RPC `error` means something about the *request itself* was wrong: an unknown method, a \
malformed body, or a tool name that doesn't exist on this endpoint.

## Budgets

Every tool runs under a budget (call count, wall-clock time, page count, concurrency) folded from \
this endpoint's own ceiling. A run that trips a budget stops rather than continuing past it; \
partial results, when any exist, are returned alongside an explanation of what didn't run.
";
