// `GET/POST/PUT/DELETE /api/endpoints[/{slug}]` and `GET /api/endpoints/{slug}/plan` — mirrors
// `src/server/api/endpoints.rs`.
import { api } from "./client";
import type { EndpointView, PackEndpoint, PlanView } from "./types";

export const endpointsApi = {
  list: (): Promise<EndpointView[]> => api.get("/api/endpoints"),
  get: (slug: string): Promise<EndpointView> => api.get(`/api/endpoints/${encodeURIComponent(slug)}`),
  // Seam for the next agent.
  create: (slug: string, def: PackEndpoint): Promise<EndpointView> =>
    api.post("/api/endpoints", { slug, ...def }),
  update: (slug: string, def: PackEndpoint): Promise<EndpointView> =>
    api.put(`/api/endpoints/${encodeURIComponent(slug)}`, def),
  remove: (slug: string): Promise<void> => api.delete(`/api/endpoints/${encodeURIComponent(slug)}`),
  /** The resolved tool list + reachable-origin set — what `tools/list` actually returns. */
  plan: (slug: string): Promise<PlanView> => api.get(`/api/endpoints/${encodeURIComponent(slug)}/plan`),
};
