// `ParamLocation` is `"path" | "query" | "header" | "local" | { body: string }` on the wire — the
// one variant that isn't a bare string needs its own kind/detail split to drive a plain <select>.
import type { ParamLocation } from "@/api";

export type LocationKind = "path" | "query" | "header" | "local" | "body";

export function locationKind(loc: ParamLocation): LocationKind {
  return typeof loc === "string" ? loc : "body";
}

export function bodyPointer(loc: ParamLocation): string {
  return typeof loc === "string" ? "" : loc.body;
}

export function buildLocation(kind: LocationKind, pointer: string): ParamLocation {
  return kind === "body" ? { body: pointer } : kind;
}

export const LOCATION_LABELS: Record<LocationKind, string> = {
  path: "path",
  query: "query",
  header: "header",
  body: "body",
  local: "local (script input only)",
};
