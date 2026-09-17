// `POST/GET /api/tokens`, `DELETE /api/tokens/{id}` — self-serve MCP access tokens. Mirrors
// whatever route the server side of this chunk lands under `src/server/api/tokens.rs`.
import { api } from "./client";
import type { TokenCreateRequest, TokenCreateResponse, TokenView } from "./types";

export const tokensApi = {
  list: (): Promise<TokenView[]> => api.get("/api/tokens"),
  create: (body: TokenCreateRequest): Promise<TokenCreateResponse> => api.post("/api/tokens", body),
  revoke: (id: string): Promise<void> => api.delete(`/api/tokens/${encodeURIComponent(id)}`),
};
