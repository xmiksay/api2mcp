// `GET /api/runs` (filtered, paged) and `GET /api/runs/{id}` — mirrors `src/server/api/runs.rs`.
import { api } from "./client";
import type { RunDetailView, RunFilterParams, RunSummaryView } from "./types";

function query(params: RunFilterParams): string {
  const sp = new URLSearchParams();
  if (params.endpoint) sp.set("endpoint", params.endpoint);
  if (params.status) sp.set("status", params.status);
  if (params.limit !== undefined) sp.set("limit", String(params.limit));
  if (params.offset !== undefined) sp.set("offset", String(params.offset));
  const s = sp.toString();
  return s ? `?${s}` : "";
}

export const runsApi = {
  list: (params: RunFilterParams = {}): Promise<RunSummaryView[]> =>
    api.get(`/api/runs${query(params)}`),
  get: (id: string): Promise<RunDetailView> => api.get(`/api/runs/${encodeURIComponent(id)}`),
};
