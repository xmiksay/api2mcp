import { describe, expect, it } from "vitest";
import type { PackEndpoint } from "@/api";
import { endpointPayload } from "./endpointForm";

function form(internal: string | null | undefined): PackEndpoint {
  return {
    tag_expr: "has(demo)",
    internal_tag_expr: internal,
    write_ceiling: "read",
    budgets: {},
    enabled: true,
    aliases: {},
    auth_providers: [],
  };
}

describe("endpointPayload", () => {
  it("drops a blank internal tag expression", () => {
    for (const blank of ["", "   ", null, undefined]) {
      const body = JSON.parse(JSON.stringify(endpointPayload(form(blank))));
      expect(body).not.toHaveProperty("internal_tag_expr");
    }
  });

  it("keeps a set internal tag expression, trimmed", () => {
    expect(endpointPayload(form("  has(raw) ")).internal_tag_expr).toBe("has(raw)");
  });

  it("leaves every other field untouched", () => {
    const input = form("has(raw)");
    expect(endpointPayload(input)).toEqual({ ...input, internal_tag_expr: "has(raw)" });
  });
});
