// `GET/POST/PUT/DELETE /api/services[/{slug}]` — mirrors `src/server/api/services.rs`.
import { api } from "./client";
import type { PackService, ServiceView } from "./types";

export const servicesApi = {
  list: (): Promise<ServiceView[]> => api.get("/api/services"),
  get: (slug: string): Promise<ServiceView> => api.get(`/api/services/${encodeURIComponent(slug)}`),
  // Seam for the next agent: wire these into a "New service" / "Edit" form.
  create: (slug: string, def: PackService): Promise<ServiceView> =>
    api.post("/api/services", { slug, ...def }),
  update: (slug: string, def: PackService): Promise<ServiceView> =>
    api.put(`/api/services/${encodeURIComponent(slug)}`, def),
  remove: (slug: string): Promise<void> => api.delete(`/api/services/${encodeURIComponent(slug)}`),
};
