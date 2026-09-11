//! Heuristic detection of credential-shaped values in fields a pack author fully controls —
//! `credential_env_key`, `value_template`, and every free-form string reachable from a param's
//! `default`/`fixed`, an api_call's `query_fixed`/`body_template`, or a service's
//! `default_headers`. This is the backstop `plan.md` §6 calls for ("a pack never contains a
//! credential, nor a reference to one"): the *type* already rules out an actual secret-value
//! field existing anywhere in [`super::Pack`], so the only way a credential can leak into a pack
//! is a human pasting one into a string field that wasn't meant to hold one.
//!
//! Deliberately a heuristic, not a proof: pattern-matching over free text can neither prove
//! absence (a credential could be shaped like ordinary prose) nor prove presence (a long
//! random-looking string can be a legitimate non-secret value, e.g. a hash). It exists to catch
//! the common, careless case — pasting a live token where a template or an env var name
//! belongs — not to replace a human's review of an imported pack.

use serde_json::Value;

use super::ValidationError;

/// Prefixes used by widely-deployed token formats. Matching only needs to name the *shape*, not
/// prove the string is currently live — "never a credential, nor a reference to one" is about
/// presence, not liveness.
const KNOWN_PREFIXES: &[&str] = &[
    "sk-",
    "pk_live_",
    "sk_live_",
    "rk_live_", // Stripe-shaped
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_", // GitHub-shaped
    "xox",         // Slack-shaped
    "AKIA",
    "ASIA", // AWS-shaped
    "eyJ",  // a base64url JWT always starts with its header's `{"alg":` encoded this way
];

pub(super) fn looks_like_credential(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.len() < 12 {
        return false;
    }
    if KNOWN_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
        return true;
    }
    // A generic high-entropy fallback: long, unspaced, mixing letters and digits, and not
    // obviously a URL or a `{placeholder}` template — both legitimately look long and unspaced
    // too, so ruling them out first keeps this from flagging ordinary `value_template` strings.
    if trimmed.contains(' ') || trimmed.contains("://") || trimmed.contains('{') {
        return false;
    }
    let long_enough = trimmed.len() >= 24;
    let alnum_only = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    let has_letter = trimmed.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = trimmed.chars().any(|c| c.is_ascii_digit());
    long_enough && alnum_only && has_letter && has_digit
}

fn check(errors: &mut Vec<ValidationError>, location: &str, field: &str, value: &str) {
    if looks_like_credential(value) {
        errors.push(ValidationError::Credential {
            location: location.to_owned(),
            field: field.to_owned(),
        });
    }
}

/// Walks a JSON value's string leaves — a param's `default`/`fixed`, or a `body_template`, can
/// nest a credential-shaped string arbitrarily deep inside an object or array.
fn check_json(errors: &mut Vec<ValidationError>, location: &str, field: &str, value: &Value) {
    match value {
        Value::String(s) => check(errors, location, field, s),
        Value::Array(items) => {
            for item in items {
                check_json(errors, location, field, item);
            }
        }
        Value::Object(map) => {
            for v in map.values() {
                check_json(errors, location, field, v);
            }
        }
        _ => {}
    }
}

pub(super) fn scan(pack: &super::Pack, errors: &mut Vec<ValidationError>) {
    for (slug, svc) in &pack.services {
        let location = format!("services.{slug}");
        for v in svc.default_headers.values() {
            check(errors, &location, "default_headers", v);
        }
    }
    for (slug, provider) in &pack.auth_providers {
        let location = format!("auth_providers.{slug}");
        check(
            errors,
            &location,
            "credential_env_key",
            &provider.credential_env_key,
        );
        check(
            errors,
            &location,
            "value_template",
            &provider.value_template,
        );
    }
    for (slug, call) in &pack.api_calls {
        let location = format!("api_calls.{slug}");
        for v in call.query_fixed.values() {
            check(errors, &location, "query_fixed", v);
        }
        if let Some(body) = &call.body_template {
            check_json(errors, &location, "body_template", body);
        }
        for p in &call.params {
            if let Some(v) = &p.default {
                check_json(errors, &location, "params.default", v);
            }
            if let Some(v) = &p.fixed {
                check_json(errors, &location, "params.fixed", v);
            }
        }
    }
    for (slug, script) in &pack.scripts {
        let location = format!("scripts.{slug}");
        for p in &script.params {
            if let Some(v) = &p.default {
                check_json(errors, &location, "params.default", v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_known_token_prefixes() {
        assert!(looks_like_credential("ghp_aBcDeFgHiJkLmNoPqRsT1234567890"));
        assert!(looks_like_credential("sk-abcdefghijklmnopqrstuvwxyz012345"));
        assert!(looks_like_credential(
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0"
        ));
    }

    #[test]
    fn recognises_a_generic_long_high_entropy_string() {
        assert!(looks_like_credential("aB3dE9fG2hJ4kL6mN8pQ0rS2tU4vW6x"));
    }

    #[test]
    fn does_not_flag_short_or_ordinary_strings() {
        assert!(!looks_like_credential("json"));
        assert!(!looks_like_credential("application/json"));
        assert!(!looks_like_credential("Bearer {token}"));
        assert!(!looks_like_credential("https://example.com/api/v1"));
        assert!(!looks_like_credential("A2M_CRED_DEMO_TOKEN"));
    }

    #[test]
    fn does_not_flag_a_normal_env_key_name() {
        assert!(!looks_like_credential("A2M_CRED_GITLAB_TOKEN"));
    }
}
