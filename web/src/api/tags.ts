// `GET /api/tags` — read-only: tags have no independent identity to write, see `tags.rs`.
import { api } from "./client";

export const tagsApi = {
  list: (): Promise<string[]> => api.get("/api/tags"),
};
