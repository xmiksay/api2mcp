//! Wire shapes for the admin JSON API's CRUD routes. Every definition body reuses
//! [`crate::pack`]'s `Pack*` types directly (`#[serde(flatten)]`ed alongside the slug, which is a
//! pack's map *key*, not a field of the value) rather than inventing a second set of near-
//! identical structs: a `PackService`/`PackApiCall`/`PackScript`/`PackEndpoint`/`PackAuthProvider`
//! is already exactly the credential-free, database-id-free document shape this API wants to
//! accept and return, and it is already `Serialize + Deserialize`. Reusing it here also means a
//! definition round-trips identically whether it arrives via `POST /api/api_calls` or
//! `api2mcp import` — one JSON shape, two entry points.
//!
//! A `Create` body carries `slug` because a pack's own map key isn't a JSON field; an `Update`
//! body doesn't — the slug is already fixed by the URL path, and this API does not support
//! renaming a definition in place (see `server::api::validate_write`'s module doc).

use serde::{Deserialize, Serialize};

use crate::pack::{PackApiCall, PackAuthProvider, PackEndpoint, PackScript, PackService};

macro_rules! slugged_dto {
    ($create:ident, $view:ident, $pack:ty) => {
        #[derive(Debug, Clone, Deserialize)]
        pub struct $create {
            pub slug: String,
            #[serde(flatten)]
            pub def: $pack,
        }

        #[derive(Debug, Clone, Serialize)]
        pub struct $view {
            pub slug: String,
            #[serde(flatten)]
            pub def: $pack,
        }
    };
}

slugged_dto!(ServiceCreate, ServiceView, PackService);
slugged_dto!(AuthProviderCreate, AuthProviderView, PackAuthProvider);
slugged_dto!(ApiCallCreate, ApiCallView, PackApiCall);
slugged_dto!(ScriptCreate, ScriptView, PackScript);
slugged_dto!(EndpointCreate, EndpointView, PackEndpoint);
