# Architecture

## Stack

| | |
|---|---|
| Language | Rust 2024, single crate, modules only |
| Web | Axum 0.8 + Tokio |
| Persistence | SeaORM 1.1 + PostgreSQL |
| Upstream HTTP | reqwest 0.12 with a custom guarded DNS resolver |
| Scripting | Rhai 1.x, `new_raw()` with a hand-assembled package set |
| Projection | `serde_json_path` (RFC 9535) |
| Frontend | Vue 3 + TypeScript + Vite + Tailwind 4 + Pinia, embedded via `rust-embed` |
| Errors | `anyhow` above the model boundary, `thiserror` below it |
| Logging / CLI | `tracing` / `clap` |

## The central idea

There is a **compile step** between "rows in Postgres" and "execution": `resolve::EndpointPlan`.

```
rows (or an imported YAML pack)
        │
        ▼
   resolve::build_plan ───► every statically checkable invariant is decided HERE
        │                    I1 declared-calls  I2 reachable origins
        │                    I3 template compile  I5 auth↔origin binding
        │                    I6 budget folding
        ▼
  EndpointPlan (immutable, cached, generation-guarded)
        │
        ├──► server::mcp     tools/list, tools/call, invoke
        ├──► script          Rhai, api() / api_many()
        └──► cli             api2mcp call / script run
                 │
                 ▼
            runtime::Executor ──► http:: bind → guard → auth → send → body → project
                 │
                 ▼
            runs + run_calls (definition snapshot, redacted I/O, per-call timings)
```

The executor re-checks the cheap invariants at send time as defence in depth, but it never
*computes* them. A half-valid endpoint never serves: any failure in `resolve` fails the whole plan.

## Layering rule

`sea_orm` is a dependency of exactly three modules — `entity/`, `migration/`, `store/`. Stores
return `model::` types; an `entity::Model` must never escape `store/`.

Everything above that line (`resolve`, `runtime`, `http`, `script`, `schema`, `project`, `pack`) is
pure over hand-built structs and unit-testable with no database. That is what keeps the unit suite
fast and the integration suite small, and it is worth defending against convenience.

## Modules

| Module | Responsibility |
|---|---|
| `config` | Resolved process config. `from_env` → `from_lookup` seam so parsing is testable without touching the process environment. |
| `db` | Connection, and migrations run under a Postgres advisory lock (SeaORM takes none of its own, so two replicas would race). |
| `observe` | Tracing setup and the request span. Records no headers, ever. |
| `model` | Domain types. No `sea_orm`, no clock, no I/O. |
| `schema` | Generates the MCP `inputSchema` from the typed param list, and binds caller arguments against that list — never against the generated schema. |
| `http` | The upstream client. Where I2, I3 and I4 are physically enforced. |
| `secret` | `Secret`: no `Debug`/`Display`/`Serialize`, one `pub(crate)` exit into a sensitive `HeaderValue`. |
| `project` | Declarative JSONPath projection. |
| `resolve` | Rows → validated `EndpointPlan`. The invariant chokepoint. |
| `runtime` | Budgets, dispatch, fan-out, partial failure, the audit record. |
| `script` | Rhai: sandboxed engine, the `api`/`api_many` bindings, the sync/async bridge. |
| `store` | Per-aggregate façades over the database. |
| `entity`, `migration` | SeaORM entities and in-crate migrations. |
| `pack` | Portable YAML export/import. |
| `server` | MCP data plane, OAuth 2.1 AS, read-only JSON API, embedded SPA. |
| `cli` | Local-process entry points, including everything an agent must never be able to do. |

## Data model

Five definition entities plus identity and audit.

| Entity | Holds |
|---|---|
| `service` | Base URL, origin allowlist, rate limit, default headers, timeouts, concurrency. The thing that changes when a pack is shared. |
| `auth_provider` | Kind (bearer / api key / OAuth), the env **key name** of the credential, declared scopes, and the origin it is bound to. |
| `api_call` | One operation: method, path template, typed params, projection, read/write class, idempotency, tags. |
| `script` | A Rhai composition, its declared api_calls, its budgets. |
| `endpoint` | What an agent sees: a tag expression over api_calls and scripts, aliases, budgets, a read/write ceiling. |

Grouping is by **tags, many-to-many** — not a tree. A tag comes from the service, the domain and
the read/write class; an endpoint selects with an expression over them.

**No column anywhere can hold a credential value** — only `credential_env_key`, an env var name.
That is invariant I4's structural half. `script_api_calls` is I1's declarative half.

List-valued columns (`origin_allowlist`, `scopes`, `redirect_uris`) are **`JSONB`, not `TEXT[]`**.
SeaORM's Postgres array support sits behind a feature flag, and the schema is already JSONB-heavy
(`query_fixed`, `body_template`, `projection`, `pagination`, `errors`, `definition_snapshot`), so
one representation for every structured column is the simpler thing to hold in your head. These
lists are always read whole and never queried by element, so the GIN-indexability of a real array
buys nothing here. Switching later needs a migration — the two are wire-incompatible.

`runs` stores a full `definition_snapshot` of what actually executed plus a `definition_digest`.
Because definitions are mutable (see below), that snapshot is the *only* answer to "what did this
tool look like when it ran", so it carries the api_call/script, its params, its projection, the
service minus credentials, and the folded budgets. `run_calls.seq` is the **input** index of a
fan-out, never the completion index — invariant I7 made durable.

## Invariants

The seven invariants and the exact function enforcing each are in the crate-level doc comment in
`src/lib.rs`. Read it before touching `http/`, `resolve/`, `runtime/` or `script/`.

**I7 is no longer absolute.** Ordering and budget attribution are unconditionally deterministic —
input-order fan-out assembly, whole-batch call reservation, index-order byte commit — and that half
is what `run_calls.seq` records. But scripts need real date logic to be useful for transformation,
so time enters through two separate doors: `execution_start()`, frozen for the run and recorded on
it, and `now()`, the live wall clock. A script built only on the former is reproducible and
replayable from its run record; one that calls the latter is not, and that trade is visible in the
script's own source rather than hidden in the engine. `BasicTimePackage` is still excluded, and a
golden test still fails if it reappears.

`plan.md`'s **I8 — versioned, immutable definitions — is deliberately not implemented.** Definitions
are mutable and last-write-wins. This trades the ability to pin a tool version for a much smaller
write path, and pushes the entire audit burden onto the run log.

## Deliberate non-obvious choices

- **`POST /mcp/{endpoint}`**, with bare `/mcp` resolving to the configured default. `endpoint` is a
  first-class entity; a single fixed path would give it no transport.
- **Login and OAuth consent are server-rendered**, not SPA views, so the auth flow does not depend
  on `web/dist` holding a real bundle — which it does not on a fresh clone or in CI.
- **Plans are cached, registries are not.** A client calls `tools/list` on every session start;
  rebuilding a plan each time means re-querying, re-compiling every template and re-generating
  every schema. The cache is keyed on a `meta.definitions_generation` counter.
- **Redirects are followed manually** with `redirect::Policy::none()`, so every hop re-enters the
  same origin check and auth application as hop zero. Because auth is origin-bound, a cross-origin
  redirect cannot carry the first origin's credential.
- **The SSRF hook is a custom `reqwest::dns::Resolve`**, not `ClientBuilder::resolve()` — those
  overrides are applied *on top of* a custom resolver and would bypass the guard. A pre-flight
  literal-IP check is required alongside it, because hyper skips the resolver entirely for IP
  literals. `.no_proxy()` is mandatory: with a proxy configured, the proxy does the resolution.
- **Tokens are stored as sha256 hashes**, not argon2 — a token is 244 bits of randomness, so a fast
  hash is safe and a KDF on every MCP call is not. Passwords are argon2id.
