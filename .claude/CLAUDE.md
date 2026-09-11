# api2mcp — project brief

**MCP Tool Factory.** Turns curated HTTP API calls into MCP tools. A human defines a service, an
auth provider, api_calls and scripts; an endpoint selects a subset of them by tag expression; an
agent connects over MCP and sees only that subset.

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
make migrate      # apply migrations
make seed         # import examples/demo.pack.yaml
```

`DATABASE_URL` is required; copy `.env.example` to `.env`. A local Postgres on 5432 works as-is;
`make db-up` starts a containerised one on **5433** instead, so it never fights an existing server.

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
- **The Rhai host bindings are `api` / `api_many`, never `call`.** `call` is the reserved
  `KEYWORD_FN_PTR_CALL`; a same-named `register_fn` is silently shadowed.

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
