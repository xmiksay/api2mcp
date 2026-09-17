// Client-side mirror of `schema::input_schema` (src/schema/mod.rs) — the exact rule that turns a
// param list into the MCP `inputSchema` a model would see. Kept in lockstep with that function
// (filter out `fixed`, sort by `position`, alphabetical `properties` because `serde_json` is
// never built with `preserve_order`) so the live preview in the api_call/script forms shows
// precisely what the server would generate, not an approximation of it.
import type { JsonSchema, PackParam, ParamType } from "@/api";

function jsonSchemaTypeName(ty: ParamType): string {
  return ty === "string_array" ? "array" : ty;
}

function propertySchema(p: PackParam): Record<string, unknown> {
  const prop: Record<string, unknown> = { type: jsonSchemaTypeName(p.type) };
  if (p.type === "string_array") prop.items = { type: "string" };
  if (p.description !== undefined) prop.description = p.description;
  if (p.enum_values !== undefined) prop.enum = p.enum_values;
  if (p.default !== undefined) prop.default = p.default;
  return prop;
}

/** Mirrors `schema::input_schema` — a fixed param (`fixed !== undefined`) never appears. */
export function buildInputSchema(params: PackParam[]): JsonSchema {
  const visible = [...params].filter((p) => p.fixed === undefined).sort((a, b) => a.position - b.position);
  const properties: Record<string, unknown> = {};
  const required: string[] = [];
  // `properties`' own key order is alphabetical once serialized (no `preserve_order`), so sort
  // here too — otherwise the live preview would show insertion order and disagree with what the
  // server actually returns.
  for (const p of [...visible].sort((a, b) => a.name.localeCompare(b.name))) {
    properties[p.name] = propertySchema(p);
  }
  for (const p of visible) {
    if (p.required) required.push(p.name);
  }
  return { type: "object", properties, required };
}
