//! RFC 9728 protected-resource metadata and RFC 8414 authorization-server metadata. Both
//! must agree with what [`crate::server::auth::challenge`] advertises in `WWW-Authenticate` —
//! that `Bearer resource_metadata="…"` value is built from `cfg.base_url` exactly the way the
//! two documents here are, so a client that follows the 401 always lands on a document that
//! in turn points back at the same issuer.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::server::identity::SCOPE_MCP;
use crate::server::state::AppState;

pub async fn protected_resource_metadata(State(state): State<AppState>) -> Json<Value> {
    Json(protected_resource_metadata_json(&state.cfg.base_url))
}

pub async fn authorization_server_metadata(State(state): State<AppState>) -> Json<Value> {
    Json(authorization_server_metadata_json(&state.cfg.base_url))
}

fn protected_resource_metadata_json(base_url: &str) -> Value {
    json!({
        "resource": format!("{base_url}/mcp"),
        "authorization_servers": [base_url],
        "bearer_methods_supported": ["header"],
        "scopes_supported": [SCOPE_MCP],
    })
}

fn authorization_server_metadata_json(base_url: &str) -> Value {
    json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{base_url}/oauth/authorize"),
        "token_endpoint": format!("{base_url}/oauth/token"),
        "registration_endpoint": format!("{base_url}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": [SCOPE_MCP],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_resource_points_at_the_mcp_endpoint_and_this_issuer() {
        let doc = protected_resource_metadata_json("https://h.example");
        assert_eq!(doc["resource"], json!("https://h.example/mcp"));
        assert_eq!(doc["authorization_servers"], json!(["https://h.example"]));
    }

    #[test]
    fn authorization_server_metadata_agrees_with_the_issuer() {
        let doc = authorization_server_metadata_json("https://h.example");
        assert_eq!(doc["issuer"], json!("https://h.example"));
        assert_eq!(
            doc["authorization_endpoint"],
            json!("https://h.example/oauth/authorize")
        );
        assert_eq!(
            doc["token_endpoint"],
            json!("https://h.example/oauth/token")
        );
        assert_eq!(doc["code_challenge_methods_supported"], json!(["S256"]));
    }
}
