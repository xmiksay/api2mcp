//! OIDC relying-party protocol client for human login (Decision 2). This crate is a *consumer*
//! of someone else's identity provider here — a different role from [`super::oauth`], which is
//! this crate acting as an authorization *server* for MCP clients. Do not confuse the two: this
//! module never issues a token, it only ever receives one from a provider it doesn't control.
//!
//! **No JWT / JWKS dependency.** [`fetch_identity`] resolves the signed-in user by calling the
//! provider's `userinfo` endpoint with the access token, rather than verifying the ID token's
//! JWT signature. OIDC Core §3.1.3.7 permits skipping that verification when the token came
//! straight from the token endpoint over TLS — which [`exchange_code`] guarantees, since it's
//! the only caller that ever obtains one — and doing so avoids pulling in JWKS fetching and a
//! JWT library for no security gain. `Cargo.toml` is out of this chunk's file ownership in any
//! case; if a JWT dependency ever turns out to be genuinely required, that's a decision for
//! whoever owns that file, not this module.
//!
//! [`super::login`] owns the browser-facing half of the flow (the `state`/PKCE cookie, session
//! creation); this module only ever talks to the provider.

use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::OidcConfig;

/// The subset of `{issuer}/.well-known/openid-configuration` this client actually uses.
#[derive(Debug, Clone, Deserialize)]
pub struct Discovery {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
}

/// Fetches and parses the provider's discovery document (RFC 8414 / OIDC Discovery 1.0).
pub async fn discover(client: &reqwest::Client, issuer: &str) -> Result<Discovery> {
    let url = format!("{issuer}/.well-known/openid-configuration");
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?
        .error_for_status()
        .with_context(|| format!("{url} returned an error status"))?;
    response
        .json::<Discovery>()
        .await
        .with_context(|| format!("parsing {url} as OIDC discovery metadata"))
}

/// A PKCE verifier/challenge pair (RFC 7636, `S256` method only — plain `S256` support is
/// mandatory for the provider side per the OAuth 2.1 draft this crate's own AS already follows,
/// see `server::oauth`).
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// The verifier is 256 bits of CSPRNG output, base64url-encoded — the same "opaque,
/// high-entropy, server-generated" shape as [`super::auth::new_token`], which happens to also
/// satisfy RFC 7636's 43-character minimum length exactly.
pub fn generate_pkce() -> Pkce {
    let verifier = super::auth::new_token();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
    }
}

/// Builds the authorization-endpoint redirect URL: `response_type=code`, scope `openid email`
/// (`email` is not optional — [`crate::store::UserStore::find_or_create_by_oidc`] needs one to
/// create a user on first sign-in), the caller's `state`, and PKCE's `code_challenge`/`S256`.
pub fn authorize_url(
    discovery: &Discovery,
    cfg: &OidcConfig,
    state: &str,
    code_challenge: &str,
) -> Result<url::Url> {
    let mut url = url::Url::parse(&discovery.authorization_endpoint)
        .context("provider's authorization_endpoint is not a valid URL")?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &cfg.redirect_uri)
        .append_pair("scope", "openid email")
        .append_pair("state", state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url)
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// Exchanges an authorization code for an access token (RFC 6749 §4.1.3), authenticating as a
/// confidential client with `client_secret_basic` — see
/// [`crate::secret::Secret::into_basic_auth_header`]'s own doc for why that scheme and not
/// `client_secret_post`. `code_verifier` goes in the body: PKCE doesn't need header-level
/// sensitivity, since the value it's checked against (`code_challenge`) already crossed the
/// network once, in the authorize redirect.
pub async fn exchange_code(
    client: &reqwest::Client,
    discovery: &Discovery,
    cfg: &OidcConfig,
    code: &str,
    code_verifier: &str,
) -> Result<String> {
    let auth_header = cfg
        .client_secret
        .clone()
        .into_basic_auth_header(&cfg.client_id)
        .context("building the token request's Basic auth header")?;
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", cfg.redirect_uri.as_str()),
        ("code_verifier", code_verifier),
        ("client_id", cfg.client_id.as_str()),
    ];
    let response = client
        .post(&discovery.token_endpoint)
        .header(reqwest::header::AUTHORIZATION, auth_header)
        .form(&params)
        .send()
        .await
        .context("sending the token request")?
        .error_for_status()
        .context("token endpoint returned an error status")?;
    let body: TokenResponse = response
        .json()
        .await
        .context("parsing the token endpoint's response")?;
    Ok(body.access_token)
}

#[derive(Debug, Deserialize)]
struct UserInfoResponse {
    sub: String,
    email: Option<String>,
}

/// The identity [`super::login`]'s callback resolves a user from.
pub struct Identity {
    pub subject: String,
    pub email: String,
}

/// Calls the `userinfo` endpoint with the access token (OIDC Core §5.3) — see this module's own
/// doc for why this, rather than an ID token signature check, is how the identity is trusted.
pub async fn fetch_identity(
    client: &reqwest::Client,
    discovery: &Discovery,
    access_token: &str,
) -> Result<Identity> {
    let response = client
        .get(&discovery.userinfo_endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .context("sending the userinfo request")?
        .error_for_status()
        .context("userinfo endpoint returned an error status")?;
    let body: UserInfoResponse = response
        .json()
        .await
        .context("parsing the userinfo response")?;
    let email = body
        .email
        .ok_or_else(|| anyhow!("provider's userinfo response carried no email"))?;
    Ok(Identity {
        subject: body.sub,
        email,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::Secret;

    fn test_cfg() -> OidcConfig {
        OidcConfig {
            issuer: "https://idp.example.com".into(),
            client_id: "client-1".into(),
            client_secret: Secret::from_raw("shh".into()),
            redirect_uri: "https://tools.example.com/login/oidc/callback".into(),
        }
    }

    fn test_discovery() -> Discovery {
        Discovery {
            authorization_endpoint: "https://idp.example.com/authorize".into(),
            token_endpoint: "https://idp.example.com/token".into(),
            userinfo_endpoint: "https://idp.example.com/userinfo".into(),
        }
    }

    #[test]
    fn pkce_challenge_matches_the_rfc_7636_appendix_b_test_vector() {
        // https://www.rfc-editor.org/rfc/rfc7636#appendix-B
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn generated_pkce_pairs_are_unique_and_the_right_length() {
        let a = generate_pkce();
        let b = generate_pkce();
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.challenge, b.challenge);
        assert_eq!(a.verifier.len(), 43);
        assert_eq!(a.challenge.len(), 43);
    }

    #[test]
    fn authorize_url_carries_every_required_parameter() {
        let url = authorize_url(&test_discovery(), &test_cfg(), "the-state", "the-challenge")
            .expect("valid url");
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(pairs["response_type"], "code");
        assert_eq!(pairs["client_id"], "client-1");
        assert_eq!(
            pairs["redirect_uri"],
            "https://tools.example.com/login/oidc/callback"
        );
        assert_eq!(pairs["scope"], "openid email");
        assert_eq!(pairs["state"], "the-state");
        assert_eq!(pairs["code_challenge"], "the-challenge");
        assert_eq!(pairs["code_challenge_method"], "S256");
    }
}
