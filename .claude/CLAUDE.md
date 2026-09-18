# api2mcp — project brief

**MCP Tool Factory.** Turns curated HTTP API calls into MCP tools. A human defines a service, at
most one auth provider for it, api_calls and scripts; an endpoint selects a subset of them by tag
expression; an agent connects over MCP and sees only that subset.

The value is **persistence, determinism, narrowing and auditability** — not reach. An agent with a
generic HTTP tool can already call any API; what it cannot do is call a *specific* one the same way
every time, with a bounded blast radius and a record of what happened.

See [`../plan.md`](../plan.md) for the design document and [`../docs/architecture.md`](../docs/architecture.md)
for the module-level map.

## Build & run

Everything goes through `make` — see `make help`. Never retype the underlying cargo/npm commands.

```
make check        # fast typecheck, no SPA build
make verify       # THE pre-"done" gate: lint + all tests
make run          # server on :8080
make dev          # vite on :5173 proxying /api, /mcp, /oauth, /login to :8080
make db-create    # create the role and database (idempotent)
make migrate      # apply migrations
make seed         # import examples/demo.pack.yaml
```

`DATABASE_URL` is required; copy `.env.example` to `.env`. The database is a **Postgres you run
locally** — there is no container for it. `make db-create` sets up the role and database
idempotently; `make db-reset` drops and recreates it.

## Auth & ownership

**There is no admin.** `users.is_admin`, `assert_admin` and `SCOPE_ADMIN` do not exist. A signed-in
session can read and write every definition *it owns*, full stop — see `server::identity`'s
module doc. A service token or an OAuth-authenticated MCP client can only call tools over `/mcp`;
neither can construct a `Caller` that reaches `/api/*` (`Caller`'s `FromRequestParts` impl resolves
exclusively from the session cookie and never inspects `Authorization` at all — a structural
property of the router, not a per-route check).

**Human login is a password form, an external OIDC provider, or both** (`server::login`,
`server::login_oidc`, `server::oidc`), configured entirely through `A2M_OIDC_ISSUER` /
`A2M_OIDC_CLIENT_ID` / `A2M_OIDC_CLIENT_SECRET` — all three or none. No client secret ever lives in
the database. A returning OIDC user is matched on `(issuer, subject)`, **never** on email (email is
mutable, provider-side); an account with no identity bound yet (a password account, or one
pre-provisioned with `api2mcp user add --oidc-only`) is claimed on first sign-in only when the
provider asserts `email_verified: true` and the email matches exactly.

**Auth belongs to the service, not the api_call.** `auth_providers.service_id` is `UNIQUE` — a
service has at most one provider, and every api_call on that service uses it automatically; an
api_call names no provider of its own (`model::ApiCall` has no such field at all). An api_call on
a provider-less service simply sends no credential — a normal state (exactly what a freshly
imported pack looks like), not a validation failure. `resolve::auth_bind` still enforces I5 (the
provider's `bound_origin` must match the service's origin), once per service now rather than once
per api_call.

**A credential is either an env var name or a stored value.** `auth_providers` names exactly one
source: `credential_env_key` (a variable the server reads at send time) or `credential_value` (the
credential itself, in **plaintext**, for the per-owner case — each owner needs their own token for
the same upstream, which a process-wide variable cannot express). The accepted trade is that a
database dump carries every stored credential. I4 is unaffected: it forbids a credential reaching
a model-visible surface, not persistence — the API returns only `has_stored_credential`, and the
MCP control plane cannot see auth providers at all.

**A pack carries no auth providers at all** — not the definitions, and not even a reference to one
from an api_call (`PackApiCall` has no `auth_provider` field; `Pack` has no `auth_providers` map,
and `#[serde(deny_unknown_fields)]` rejects one from an older pack outright rather than silently
ignoring it). Importing a pack never creates or touches a credential; a provider is wired up
afterwards, per service, directly through `/api/auth_providers` or the CLI.

**Every definition is owned.** `owner_id` sits on `services`, `auth_providers`, `api_calls`,
`scripts`, `endpoints`, `runs` and `service_tokens`. Slugs are unique **per owner**
(`ux_*_owner_slug`), not globally — two different people's `demo` endpoints coexist fine.
Sharing between owners is out-of-band, via a YAML pack, never a shared row — export/import is
both a CLI pair (`api2mcp export`/`api2mcp import`) and an HTTP pair
(`GET /api/endpoints/{slug}/pack`, `POST /api/packs/import`), both calling the identical
`pack::{export_endpoint,validate,import}` functions: one format, several entry points.

**Self-service tokens.** `service_tokens.scopes` is gone — a resolved, unrevoked, unexpired token
may call tools, full stop. `POST/GET /api/tokens`, `DELETE /api/tokens/{id}` let a signed-in user
mint/list/revoke their own tokens (mirrors `api2mcp token mint/list/revoke` on the CLI), with
per-endpoint grants (`service_token_endpoints`): empty means every endpoint, non-empty means
exactly those. `restricted` is a real column, not inferred from an empty grant set — deleting a
granted endpoint cascades its grant rows away, and inferring "restricted" from "has grants" would
silently *widen* a token to unrestricted the moment its one granted endpoint was deleted.

The read-write admin JSON API and its test-run routes (`POST /api/api_calls/{slug}/test`,
`POST /api/scripts/{slug}/test`) live under `server::api`; see
[`docs/architecture.md`](../docs/architecture.md) for the full route table.

## Gotchas

- **The SPA is embedded by `rust-embed` from `web/dist`,** which is a *compile-time* dependency.
  `build.rs` writes a placeholder `index.html` before anything else, so a clean clone compiles with
  no Node installed, then builds the real bundle. A failed UI build is a `panic!` in release and a
  warning in debug.
- **`SKIP_UI_BUILD=1` skips the npm step.** Every make target that does not need the bundle sets
  it. Use it whenever you run cargo by hand.
- **`rust-embed` reads from disk in debug builds**, so after `make ui` the new assets are served
  without recompiling.
- **`serde_json` must never enable `preserve_order`.** `rhai::Map` is a `BTreeMap` and default
  `serde_json::Map` is too, so both sort identically and every Rhai boundary round-trips in stable
  order. A unit test guards this.
- **Rhai gets a hand-assembled package set, never `StandardPackage`** — which includes
  `BasicTimePackage`, i.e. a wall clock inside a script. A golden test over the engine's registered
  function signatures catches an accidental re-addition.
- **The Rhai host bindings are `api` / `api_many` / `api_try`, never `call`.** `call` is the
  reserved `KEYWORD_FN_PTR_CALL`; a same-named `register_fn` is silently shadowed rather than
  rejected. See [`docs/scripting.md`](../docs/scripting.md) for the full script-callable surface.
- **Never run `vue-tsc --noEmit` directly against `web/`.** The project is `composite: true`
  (project references), so a bare `--noEmit` invocation can exit `0` while silently checking
  nothing. Always go through `npm run typecheck` (`vue-tsc -b --force`) or `make lint`.
- **Slugs are unique per owner, not globally.** `ux_*_owner_slug` on `services`/`auth_providers`/
  `api_calls`/`scripts`/`endpoints` — a lookup or a pack import must always scope by `owner_id`,
  never assume a bare slug is globally unique.

## Conventions & invariants

The seven invariants and their physical enforcement points are documented in the crate-level doc
comment in `src/lib.rs` — read it before touching `http/`, `resolve/`, `runtime/` or `script/`.

`plan.md`'s I8 (versioned, immutable definitions) is deliberately **not** implemented. Definitions
are mutable and last-write-wins; history lives in the run log, where every run stores a complete
snapshot of the definition that actually executed.

- **Layering.** `sea_orm` is a dependency of exactly three modules: `entity/`, `migration/`,
  `store/`. Stores return `model::` types — an `entity::Model` must never escape `store/`, or the
  layers above it stop being testable without Postgres.
- **`anyhow` above the model boundary, typed errors below it.** A `String` context can smuggle a
  URL with userinfo into a tool response, so anything model-visible is a `thiserror` enum with
  `Serialize`.
- **No `.unwrap()`/`.expect()`/`panic!`** on a path reachable from I/O, config, network or the DB.
- **400-line cap per file.** Split on a natural seam; never grow a file already over it.
- **`BTreeMap`/`BTreeSet`, never `HashMap`/`HashSet`,** anywhere iteration order can be observed.
- Tests ship with the change: inline `#[cfg(test)]` for pure logic, `tests/` for HTTP/DB flows.
  Integration tests skip themselves when `TEST_DATABASE_URL` is unset.

## Commits

Conventional Commits (`<type>(<scope>): <subject>`), see `.gitmessage`. Never a `Co-Authored-By`
trailer or any Claude attribution.
