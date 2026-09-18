// `GET/POST/PUT/DELETE /api/auth_providers[/{slug}]` — mirrors `src/server/api/auth_providers.rs`.
// Addressed by its bare slug: `auth_providers.slug` is unique globally, not per-service.
import { api } from "./client";
import type { AuthProviderView, AuthProviderWrite } from "./types";

export const authProvidersApi = {
  list: (): Promise<AuthProviderView[]> => api.get("/api/auth_providers"),
  get: (slug: string): Promise<AuthProviderView> =>
    api.get(`/api/auth_providers/${encodeURIComponent(slug)}`),
  // Seam for the next agent. Note the server rejects moving a provider to a different service
  // via PUT — see `auth_providers.rs`'s module doc.
  create: (slug: string, def: AuthProviderWrite): Promise<AuthProviderView> =>
    api.post("/api/auth_providers", { slug, ...def }),
  update: (slug: string, def: AuthProviderWrite): Promise<AuthProviderView> =>
    api.put(`/api/auth_providers/${encodeURIComponent(slug)}`, def),
  remove: (slug: string): Promise<void> =>
    api.delete(`/api/auth_providers/${encodeURIComponent(slug)}`),
};
