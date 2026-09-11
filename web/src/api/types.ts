// Wire shapes for `/api/*`, mirrored field-for-field from `src/pack/mod.rs` (the `Pack*` types,
// which every CRUD DTO in `src/server/api/dto.rs` flattens `slug` onto) and from the read-only
// views in `src/server/api/{endpoints,runs,health,me}.rs`. Do not add a field that isn't actually
// serialized on the Rust side — this module is the contract the rest of the app trusts blindly.

export type Access = "read" | "write";

export type ParamType = "string" | "integer" | "number" | "boolean" | "string_array";

// `PackParamLocation` is a plain externally-tagged enum: unit variants serialize as a bare
// string, the one payload-carrying variant (`Body`) as `{ "body": "<json pointer>" }`.
export type ParamLocation = "path" | "query" | "header" | "local" | { body: string };

export interface PackParam {
  name: string;
  location: ParamLocation;
  /** Renamed from Rust's `ty` — the wire field is `type`. */
  type: ParamType;
  required: boolean;
  default?: unknown;
  /** Set by the definer; a caller can never supply or see this (`Param::is_model_visible`). */
  fixed?: unknown;
  enum_values?: unknown[];
  description?: string;
  position: number;
}

export type Cardinality = "one" | "many";

export interface PackProjectionField {
  name: string;
  path: string;
  cardinality: Cardinality;
  coerce?: ParamType;
}

export interface PackProjection {
  fields: PackProjectionField[];
}

// `PackPagination` is internally tagged on `kind`.
export type PackPagination =
  | { kind: "none" }
  | { kind: "cursor"; next_cursor_path: string; query_param: string };

export interface PackBudgets {
  max_calls?: number;
  max_bytes?: number;
  wall_clock_ms?: number;
  max_pages?: number;
  max_concurrency?: number;
}

export interface PackService {
  base_url: string;
  origin_allowlist: string[];
  default_headers: Record<string, string>;
  timeout_ms: number;
  max_concurrency: number;
  rate_limit_per_min?: number;
  max_response_bytes: number;
}
export interface ServiceView extends PackService {
  slug: string;
}

export type PackAuthKind = "static_header" | "oauth2_client_credentials";

export interface PackAuthProvider {
  service: string;
  kind: PackAuthKind;
  /** An env var *name*, never a credential value — see `pack::mod`'s module doc. */
  credential_env_key: string;
  header_name: string;
  value_template: string;
  scopes: string[];
  token_url?: string;
  bound_origin: string;
}
export interface AuthProviderView extends PackAuthProvider {
  slug: string;
}

export interface PackApiCall {
  service: string;
  auth_provider?: string;
  method: string;
  path_template: string;
  query_fixed: Record<string, string>;
  body_template?: unknown;
  access: Access;
  idempotent: boolean;
  projection?: PackProjection;
  pagination: PackPagination;
  timeout_ms?: number;
  max_response_bytes?: number;
  params: PackParam[];
  tags: string[];
  description?: string;
}
export interface ApiCallView extends PackApiCall {
  slug: string;
}

export interface PackScript {
  source: string;
  params: PackParam[];
  /** alias (used inside `api()`/`api_many()`) -> api_call slug. */
  callable: Record<string, string>;
  budgets: PackBudgets;
  description?: string;
  tags: string[];
}
export interface ScriptView extends PackScript {
  slug: string;
}

// `PackEndpointTarget` is externally tagged: `{ "api_call": "<slug>" }` or `{ "script": "<slug>" }`.
export type PackEndpointTarget = { api_call: string } | { script: string };

export interface PackEndpoint {
  tag_expr: string;
  write_ceiling: Access;
  budgets: PackBudgets;
  instructions?: string;
  enabled: boolean;
  /** alias -> target: renames a tool's exposed name away from its own slug. */
  aliases: Record<string, PackEndpointTarget>;
  auth_providers: string[];
}
export interface EndpointView extends PackEndpoint {
  slug: string;
}

// --- GET /api/endpoints/{slug}/plan -----------------------------------------------------------

export interface BudgetsView {
  max_calls: number | null;
  max_bytes: number | null;
  wall_clock_ms: number | null;
  max_pages: number | null;
  max_concurrency: number | null;
}

export interface ToolView {
  name: string;
  input_schema: JsonSchema;
  target_kind: "api_call" | "script";
  target_slug: string;
  budgets: BudgetsView;
}

export interface PlanView {
  slug: string;
  write_ceiling: Access;
  instructions: string | null;
  digest: string;
  /** The statically computed reachable-origin set (I2) — see `endpoints.rs`'s own doc. */
  origins: string[];
  budgets: BudgetsView;
  tools: ToolView[];
}

/** A generated JSON Schema document — rendered, not interpreted, so it stays untyped beyond this. */
export type JsonSchema = Record<string, unknown>;

// --- GET /api/health ---------------------------------------------------------------------------

export interface HealthView {
  version: string;
  commit: string;
  db_connected: boolean;
  migrations_applied: number;
  migrations_total: number;
  endpoint_count: number;
}

// --- GET /api/me ---------------------------------------------------------------------------------

export interface MeView {
  id: string;
  kind: "session" | "service_token" | "cli";
  is_admin: boolean;
}

// --- Runs (the audit trail) ----------------------------------------------------------------------

export type RunStatus = "ok" | "partial" | "error" | "denied" | "budget_exceeded" | "timeout";
export type RunTargetKind = "api_call" | "script";

export interface RunSummaryView {
  id: string;
  endpoint_slug: string;
  tool_name: string;
  target_kind: RunTargetKind;
  target_slug: string;
  status: RunStatus;
  calls_made: number;
  bytes_in: number;
  pages_fetched: number;
  created_at: string;
}

export interface RunCallView {
  /** Input index of a fan-out, not completion order — never re-sort this. */
  seq: number;
  api_call_slug: string;
  service_slug: string;
  status_code: number | null;
  response_bytes: number | null;
  response_truncated: boolean;
  error: string | null;
  /** The upstream's own (redacted) response body for this call, straight from the audit row. */
  raw: unknown | null;
}

export interface RunDetailView extends RunSummaryView {
  calls: RunCallView[];
}

export interface RunFilterParams {
  endpoint?: string;
  status?: RunStatus;
  limit?: number;
  offset?: number;
}

// --- Test-run routes (POST /api/api_calls/{slug}/test, /api/scripts/{slug}/test) -----------------
// Not surfaced by any view this chunk builds, but declared so the next agent's test panel needs
// no client-layer work.

export type RunStatusView = RunStatus;

export interface ApiCallTestResult {
  run_id: string;
  status: RunStatusView;
  projected: unknown | null;
  raw: unknown | null;
  error: string | null;
}

export interface ScriptCallView {
  seq: number;
  api_call_slug: string;
  service_slug: string;
  status_code: number | null;
  response_bytes: number | null;
  raw: unknown | null;
}

/** Present on failure: kind, message, and — when available — line/column/source snippet. */
export interface ScriptFailure {
  kind: string;
  message: string;
  line?: number;
  column?: number;
  snippet?: string;
}

export interface ScriptTestResult {
  run_id: string;
  status: RunStatusView;
  value: unknown | null;
  calls: ScriptCallView[];
  failure: ScriptFailure | null;
}
