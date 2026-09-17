//! Shared `inputSchema` fragments for the control plane's `create`/`update` tools.
//!
//! [`crate::model::Param`]/[`crate::model::Budgets`]/[`crate::model::Projection`]/
//! [`crate::model::Pagination`] each appear inside more than one tool's arguments (an
//! api_call's `params`, a script's `params` and `budgets`, an endpoint's `budgets`), and a model
//! filling one must see the identical shape every time it appears — factored out here rather
//! than repeated per tool, the same reasoning [`crate::server::api::convert`] gives for sharing
//! the `model` <-> `pack` conversions across resources.

use serde_json::{Value, json};

/// One entry of an api_call's or script's `params` array — the wire shape of a
/// [`crate::pack::PackParam`].
pub(super) fn param_schema() -> Value {
    json!({
        "type": "object",
        "description": "One argument the tool exposes to a caller, or a fixed value the \
            definer sets and a caller never sees.",
        "properties": {
            "name": {
                "type": "string",
                "description": "Argument name as it appears in inputSchema/the script scope."
            },
            "location": {
                "description": "Where this value goes. One of the strings \"path\", \"query\", \
                    \"header\", \"local\" (a script-only input, never placed in an HTTP \
                    request) — or, to splice into the request body, an object \
                    {\"body\": \"<json pointer>\"}, e.g. {\"body\": \"/user/email\"}. A \
                    \"header\" name must be one of the fixed allow-listed header names (Accept, \
                    Accept-Language, Content-Language, If-Match, If-None-Match, \
                    Idempotency-Key, X-Request-Id, X-Correlation-Id) — never Authorization, \
                    Cookie, Host or X-Forwarded-*."
            },
            "type": {
                "enum": ["string", "integer", "number", "boolean", "string_array"]
            },
            "required": {
                "type": "boolean",
                "default": false,
                "description": "Must a caller supply this? Cannot be true together with \
                    \"fixed\"."
            },
            "default": {
                "description": "Applied when the caller omits this param. Any JSON value."
            },
            "fixed": {
                "description": "Set by you, the definer; a caller can never supply or see this \
                    param when it is set (it is dropped from the tool's own inputSchema). Use \
                    this for values like an API version or output format that must never vary \
                    per call."
            },
            "enum_values": {
                "type": "array",
                "description": "Allowed values, checked after type coercion. Omit for no \
                    constraint."
            },
            "description": {
                "type": "string",
                "description": "Shown to the calling model in the tool's inputSchema."
            },
            "position": {
                "type": "integer",
                "description": "Stable ordering among this item's params; need not be \
                    contiguous."
            }
        },
        "required": ["name", "location", "type", "position"]
    })
}

pub(super) fn params_array_schema() -> Value {
    json!({
        "type": "array",
        "description": "The api_call's or script's declared parameters.",
        "items": param_schema()
    })
}

/// The wire shape of a [`crate::pack::PackProjection`] — narrows an api_call's raw upstream
/// response to just the fields a tool caller should see.
pub(super) fn projection_schema() -> Value {
    json!({
        "type": "object",
        "description": "Narrows the raw upstream JSON response to a smaller, named shape. \
            Omit entirely to pass the upstream response through unprojected.",
        "properties": {
            "fields": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Field name in the projected output."
                        },
                        "path": {
                            "type": "string",
                            "description": "A JSONPath expression into the raw upstream \
                                response, e.g. \"$.data.items[*].id\"."
                        },
                        "cardinality": {
                            "enum": ["one", "many"],
                            "description": "\"one\" takes the first match; \"many\" collects \
                                every match into an array."
                        },
                        "coerce": {
                            "enum": ["string", "integer", "number", "boolean", "string_array"],
                            "description": "Optional type coercion applied to each matched \
                                value."
                        }
                    },
                    "required": ["name", "path", "cardinality"]
                }
            }
        },
        "required": ["fields"]
    })
}

/// The wire shape of a [`crate::pack::PackPagination`].
pub(super) fn pagination_schema() -> Value {
    json!({
        "type": "object",
        "description": "How to follow multi-page responses. Omit for a single-page call.",
        "properties": {
            "kind": {"enum": ["none", "cursor"]},
            "next_cursor_path": {
                "type": "string",
                "description": "cursor only: JSON pointer to the next-page cursor in the \
                    response body, e.g. \"/next_cursor\"."
            },
            "query_param": {
                "type": "string",
                "description": "cursor only: query parameter name the next cursor is sent back \
                    on."
            }
        },
        "required": ["kind"]
    })
}

/// The wire shape of a [`crate::pack::PackBudgets`] — every field narrows (never widens) an
/// enclosing endpoint's own ceiling; see [`crate::model::Budgets::fold`].
pub(super) fn budgets_schema() -> Value {
    json!({
        "type": "object",
        "description": "Resource ceilings. Every field is optional (\"no opinion\" — the \
            enclosing endpoint's own ceiling still applies) and can only narrow an enclosing \
            ceiling, never widen it.",
        "properties": {
            "max_calls": {"type": "integer", "description": "Max upstream HTTP calls in one run."},
            "max_bytes": {"type": "integer", "description": "Max total response bytes read."},
            "wall_clock_ms": {"type": "integer", "description": "Max wall-clock run time."},
            "max_pages": {"type": "integer", "description": "Max pages followed via pagination."},
            "max_concurrency": {
                "type": "integer",
                "description": "Max concurrent upstream calls (script batches only)."
            }
        }
    })
}

pub(super) fn tags_schema() -> Value {
    json!({
        "type": "array",
        "items": {"type": "string"},
        "description": "Tag names (each a slug: lowercase letters, digits, '_', '-'). An \
            endpoint's tag_expr selects tools by matching against exactly these."
    })
}

pub(super) fn slug_schema(what: &str) -> Value {
    json!({
        "type": "string",
        "description": format!(
            "{what} slug: lowercase letters, digits, '_', '-', at most 64 bytes."
        )
    })
}

/// Merges a `slug` property into a `properties`+`required` schema fragment (as returned by e.g.
/// [`super::services::pack_service_schema`]) — the shape every `*.create`/`*.update` tool's
/// `inputSchema` needs: the pack's own fields, plus the slug identifying which definition they
/// describe (a `create`/`update` DTO's own shape — see `server::api::dto`'s module doc).
pub(super) fn with_slug(what: &str, mut base: Value) -> Value {
    if let Some(props) = base.get_mut("properties").and_then(Value::as_object_mut) {
        props.insert("slug".to_owned(), slug_schema(what));
    }
    if let Some(required) = base.get_mut("required").and_then(Value::as_array_mut) {
        required.insert(0, json!("slug"));
    }
    base
}
