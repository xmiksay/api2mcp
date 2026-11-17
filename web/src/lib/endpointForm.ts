import type { PackEndpoint } from "@/api";

// `internal_tag_expr` is optional server-side, but an empty string is not "none" there — it is
// a tag expression that fails to parse. A cleared form field must therefore be dropped from the
// body (serialized as absent), not sent as "".
export function endpointPayload(form: PackEndpoint): PackEndpoint {
  const internal = form.internal_tag_expr?.trim();
  return { ...form, internal_tag_expr: internal ? internal : undefined };
}
