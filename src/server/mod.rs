//! The Axum server: MCP data plane, OAuth 2.1 AS, read-write admin JSON API, embedded SPA.

pub mod api;
pub mod auth;
pub mod embed;
pub mod error;
pub mod identity;
pub mod login;
pub mod mcp;
pub mod oauth;
pub mod router;
pub mod state;

pub use router::build_router;
pub use state::AppState;
