// Mirrors `model::HEADER_PARAM_ALLOWLIST` (src/model/param.rs) exactly. A `header`-location
// param's name is only ever checked against this list at *dispatch* time (`http::bind`), not by
// `pack::validate` — so a disallowed name saves cleanly today and only fails the first time
// someone actually runs the tool. Constraining the picker to this list client-side is a deliberate
// compensation for that gap (see the chunk report), not a guess at future server behavior.
export const HEADER_PARAM_ALLOWLIST = [
  "Accept",
  "Accept-Language",
  "Content-Language",
  "If-Match",
  "If-None-Match",
  "Idempotency-Key",
  "X-Request-Id",
  "X-Correlation-Id",
] as const;
