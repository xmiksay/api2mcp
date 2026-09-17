// `GET /api/me` — confirms a session is live and reports the signed-in identity.
import { api } from "./client";
import type { MeView } from "./types";

export const meApi = {
  get: (): Promise<MeView> => api.get("/api/me"),
};
