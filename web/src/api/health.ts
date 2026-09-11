// `GET /api/health` — version, migration level, DB connectivity, endpoint count.
import { api } from "./client";
import type { HealthView } from "./types";

export const healthApi = {
  get: (): Promise<HealthView> => api.get("/api/health"),
};
