// `GET/POST/PUT/DELETE /api/api_calls[/{slug}]` and `POST /api/api_calls/{slug}/test` — mirrors
// `src/server/api/api_calls.rs`.
import { api } from "./client";
import type { ApiCallTestResult, ApiCallView, PackApiCall } from "./types";

export const apiCallsApi = {
  list: (): Promise<ApiCallView[]> => api.get("/api/api_calls"),
  get: (slug: string): Promise<ApiCallView> => api.get(`/api/api_calls/${encodeURIComponent(slug)}`),
  // Seam for the next agent.
  create: (slug: string, def: PackApiCall): Promise<ApiCallView> =>
    api.post("/api/api_calls", { slug, ...def }),
  update: (slug: string, def: PackApiCall): Promise<ApiCallView> =>
    api.put(`/api/api_calls/${encodeURIComponent(slug)}`, def),
  remove: (slug: string): Promise<void> => api.delete(`/api/api_calls/${encodeURIComponent(slug)}`),
  // Seam for the next agent's test panel.
  test: (slug: string, endpoint: string, args: unknown): Promise<ApiCallTestResult> =>
    api.post(`/api/api_calls/${encodeURIComponent(slug)}/test`, { endpoint, args }),
};
