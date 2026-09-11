// `GET/POST/PUT/DELETE /api/scripts[/{slug}]` and `POST /api/scripts/{slug}/test` — mirrors
// `src/server/api/scripts.rs`.
import { api } from "./client";
import type { PackScript, ScriptTestResult, ScriptView } from "./types";

export const scriptsApi = {
  list: (): Promise<ScriptView[]> => api.get("/api/scripts"),
  get: (slug: string): Promise<ScriptView> => api.get(`/api/scripts/${encodeURIComponent(slug)}`),
  // Seam for the next agent (script editor).
  create: (slug: string, def: PackScript): Promise<ScriptView> =>
    api.post("/api/scripts", { slug, ...def }),
  update: (slug: string, def: PackScript): Promise<ScriptView> =>
    api.put(`/api/scripts/${encodeURIComponent(slug)}`, def),
  remove: (slug: string): Promise<void> => api.delete(`/api/scripts/${encodeURIComponent(slug)}`),
  // Seam for the next agent's test panel.
  test: (slug: string, endpoint: string, args: unknown): Promise<ScriptTestResult> =>
    api.post(`/api/scripts/${encodeURIComponent(slug)}/test`, { endpoint, args }),
};
