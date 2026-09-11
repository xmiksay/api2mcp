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
make db-create                # role + database on your local Postgres
make migrate                  # apply the schema
make seed                     # import examples/demo.pack.yaml
make demo-upstream            # terminal 2: the fake API the demo pack curates
make run                      # terminal 3: server on :8080
```

The demo needs no credentials and no network: `make demo-upstream` serves a small catalogue on
`127.0.0.1:8089`, and the demo pack curates it. Set `A2M_ALLOW_LOOPBACK_UPSTREAM=1` so the SSRF
guard permits a loopback upstream — it refuses one by default, which is the point.

Mint a token and try it:

```bash
api2mcp token mint --label demo --scope mcp
curl -s -X POST http://127.0.0.1:8080/mcp/demo \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"get-item","arguments":{"id":"3"}}}'
```

The upstream returns an item with `owner`, `secret_internal_id` and a `_links` block; the tool
returns `{"id":"3","title":"Item 3"}`. That difference is the product. Ask for id `999` and you
get `isError: true` carrying the upstream's own 404 message rather than its error body dressed up
as data.

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
| `make db-create` / `make migrate` / `make db-reset` | create the DB / apply the schema / recreate and migrate |
| `api2mcp call <slug> --arg k=v --raw` | run one api_call, raw and projected side by side |
| `api2mcp export --endpoint <slug>` | write a portable YAML pack |

## How it is built

Single Rust crate — Axum, SeaORM/Postgres, Rhai — with a Vue 3 admin SPA embedded in the binary by
`rust-embed`, so deployment is one file. `build.rs` builds the SPA, and writes a placeholder first
so a clean clone compiles with no Node installed.

The design document is [`plan.md`](plan.md); the module map and the invariants are in
[`docs/architecture.md`](docs/architecture.md) and the crate-level docs in `src/lib.rs`.
