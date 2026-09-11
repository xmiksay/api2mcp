//! MCP JSON Schema generation and caller-argument binding.
//!
//! The typed param list is the only source of truth; the generated schema is a *rendering*
//! of it. `bind_args` never trusts the schema it emitted — a caller that ignores the schema
//! is rejected by the same code that would have rejected a caller that read it.
