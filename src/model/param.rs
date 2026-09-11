//! The parameter shape shared by `ApiCall` and `ScriptDef` — one `Param` type, not two, because
//! `schema::input_schema` and `schema::validate::bind_args` need to treat both identically.
//!
//! A design choice worth stating: `ParamLocation::Body` carries its own [`jsonptr::PointerBuf`]
//! rather than a sibling `body_path: Option<PointerBuf>` field on `Param`. The DB row has a
//! nullable `body_path` column, but folding it into the enum variant makes "a `Path` param with
//! a body path set" unrepresentable instead of merely disallowed — the model is the compiled,
//! already-valid side of the row→plan boundary, so it should rule out that state at the type
//! level rather than carry a field that a `Query`/`Header`/`Path` param must always leave `None`.

use std::str::FromStr;

use serde_json::Value;

/// Where a param's value is written into the outgoing request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamLocation {
    Path,
    Query,
    /// The header *name* is `Param::name`, checked against [`HEADER_PARAM_ALLOWLIST`] — never a
    /// caller-chosen string, since that could set `Authorization`, `Cookie`, `Host`, or
    /// `X-Forwarded-*`.
    Header,
    /// The value is spliced into the request body at this JSON pointer.
    Body(jsonptr::PointerBuf),
    /// The value is bound into the script's Rhai scope as a plain variable and never reaches an
    /// HTTP request — `script_params` deliberately has no `location` column, since a script
    /// input isn't placed anywhere in a request. Anything that binds an HTTP request (
    /// `http::bind`) must reject this variant; only `script::bindings` may accept it.
    Local,
}

/// The JSON-Schema-visible type of a param's value. Also drives coercion in
/// [`crate::schema::coerce`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
    StringArray,
}

impl ParamType {
    /// The JSON Schema `type` keyword value for scalar types; `StringArray` is `"array"` with an
    /// `items` schema, handled separately by the caller.
    pub fn json_schema_type_name(&self) -> &'static str {
        match self {
            ParamType::String => "string",
            ParamType::Integer => "integer",
            ParamType::Number => "number",
            ParamType::Boolean => "boolean",
            ParamType::StringArray => "array",
        }
    }
}

/// One caller-visible (or fixed) argument to an api_call or script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub location: ParamLocation,
    pub ty: ParamType,
    pub required: bool,
    /// Applied when the caller omits this param. Trusted as-is (set by the definer, not the
    /// caller) — never re-coerced.
    pub default: Option<Value>,
    /// Set by the definer; the caller cannot see or override it. A fixed param is never
    /// model-visible — see [`Param::is_model_visible`].
    pub fixed: Option<Value>,
    /// Allowed values, checked after coercion. Trusted as-is like `default`/`fixed`.
    pub enum_values: Option<Vec<Value>>,
    pub description: Option<String>,
    /// Stable position in the generated schema and in fan-out slot assignment (I7). Not
    /// necessarily contiguous or declaration-order.
    pub position: i32,
}

impl Param {
    /// A fixed param is set by the definer and can never be supplied or seen by the caller, so
    /// it must not appear in the generated schema's `properties` or `required`.
    pub fn is_model_visible(&self) -> bool {
        self.fixed.is_none()
    }
}

/// Header names a `Header`-location param may claim. Exact match, case-sensitive by design: a
/// caller-chosen name must spell one of these precisely, not merely case-fold to it — accepting
/// case variants would 2x the allowlist's effective surface for no benefit, since the definer
/// (a human, at publish time) controls the exact spelling.
///
/// Notably absent: `Authorization`, `Cookie`, `Host`, and anything starting `X-Forwarded-`.
pub const HEADER_PARAM_ALLOWLIST: &[&str] = &[
    "Accept",
    "Accept-Language",
    "Content-Language",
    "If-Match",
    "If-None-Match",
    "Idempotency-Key",
    "X-Request-Id",
    "X-Correlation-Id",
];

/// Whether `name` may be used as a `Header`-location param's name.
pub fn is_header_param_name_allowed(name: &str) -> bool {
    HEADER_PARAM_ALLOWLIST.contains(&name)
}

/// A validated allowed-header-name newtype, for call sites that want the check enforced by the
/// type system rather than re-checked at every use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedHeaderName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("header name {0:?} is not in HEADER_PARAM_ALLOWLIST")]
pub struct HeaderNameNotAllowed(String);

impl AllowedHeaderName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for AllowedHeaderName {
    type Err = HeaderNameNotAllowed;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if is_header_param_name_allowed(s) {
            Ok(AllowedHeaderName(s.to_owned()))
        } else {
            Err(HeaderNameNotAllowed(s.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(name: &str, position: i32, fixed: Option<Value>) -> Param {
        Param {
            name: name.to_owned(),
            location: ParamLocation::Query,
            ty: ParamType::String,
            required: false,
            default: None,
            fixed,
            enum_values: None,
            description: None,
            position,
        }
    }

    #[test]
    fn model_visible_iff_not_fixed() {
        let visible = param("q", 0, None);
        let hidden = param("secret_flag", 1, Some(Value::String("x".into())));
        assert!(visible.is_model_visible());
        assert!(!hidden.is_model_visible());
    }

    #[test]
    fn header_allowlist_rejects_authorization() {
        assert!(!is_header_param_name_allowed("Authorization"));
        assert!(!is_header_param_name_allowed("Cookie"));
        assert!(!is_header_param_name_allowed("Host"));
        assert!(!is_header_param_name_allowed("X-Forwarded-For"));
    }

    #[test]
    fn header_allowlist_accepts_allowed_name() {
        assert!(is_header_param_name_allowed("X-Request-Id"));
        assert!(is_header_param_name_allowed("Idempotency-Key"));
    }

    #[test]
    fn allowed_header_name_parses_only_allowlisted() {
        assert!("X-Request-Id".parse::<AllowedHeaderName>().is_ok());
        assert!("Authorization".parse::<AllowedHeaderName>().is_err());
    }

    #[test]
    fn body_location_carries_its_pointer() {
        let ptr = jsonptr::PointerBuf::from_tokens(["user", "email"]);
        let p = Param {
            location: ParamLocation::Body(ptr.clone()),
            ..param("email", 0, None)
        };
        match p.location {
            ParamLocation::Body(got) => assert_eq!(got, ptr),
            _ => panic!("expected Body location"),
        }
    }
}
