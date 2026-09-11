# api2mcp

Turns curated HTTP API calls into MCP tools.

An agent with a generic HTTP tool can already reach any API. What it cannot do is reach a *specific*
one the same way every time, with a bounded blast radius and a record of what happened. api2mcp is
that missing piece: you declare an API call once — its URL template, its narrowed parameter list,
the projection that trims its response — and it becomes an MCP tool whose behaviour is fixed,
whose reachable origins are computable before it runs, and whose every invocation is logged
alongside a snapshot of the definition that produced it.

Three levels of use:

1. **Declarative** — define an `api_call` and expose it as a tool.
2. **Compositional** — a Rhai script folds several `api_call`s into one model-usable answer.
3. **Portable** — definitions export as a credential-free YAML pack; the recipient supplies their
   own service URL and token.

It is explicitly **not** an aggregator of existing MCP servers, not a generic OpenAPI→MCP
generator, and not a host for anyone else's credentials.

## Quick start

```bash
cp .env.example .env          # set DATABASE_URL at minimum
make migrate                  # apply the schema
make seed                     # import examples/demo.pack.yaml
make run                      # server on :8080
```

Then point a client at it:

```bash
claude mcp add --transport http api2mcp http://localhost:8080/mcp/demo
```

## Commands

`make help` lists everything. The ones that matter day to day:

| | |
|---|---|
| `make check` | fast typecheck, no SPA build |
| `make verify` | lint + all tests — the pre-"done" gate |
| `make run` / `make dev` | server on :8080 / vite on :5173 proxying to it |
| `make migrate` / `make db-reset` | schema up / recreate and migrate |
| `api2mcp call <slug> --arg k=v --raw` | run one api_call, raw and projected side by side |
| `api2mcp export --endpoint <slug>` | write a portable YAML pack |

## How it is built

Single Rust crate — Axum, SeaORM/Postgres, Rhai — with a Vue 3 admin SPA embedded in the binary by
`rust-embed`, so deployment is one file. `build.rs` builds the SPA, and writes a placeholder first
so a clean clone compiles with no Node installed.

The design document is [`plan.md`](plan.md); the module map and the invariants are in
[`docs/architecture.md`](docs/architecture.md) and the crate-level docs in `src/lib.rs`.
