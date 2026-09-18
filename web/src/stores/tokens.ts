// Tokens don't fit `createResourceStore`'s shape: it's id-keyed (not slug), and `create`'s
// response carries a plaintext field (`token`) that the list view's `TokenView` never has. That
// plaintext is deliberately not part of this store's state — see `create` below.
import { ref } from "vue";
import { defineStore } from "pinia";
import { ApiError, tokensApi } from "@/api";
import type { TokenCreateRequest, TokenCreateResponse, TokenView } from "@/api";

function messageOf(e: unknown): string {
  return e instanceof ApiError ? e.message : "request failed";
}

export const useTokensStore = defineStore("tokens", () => {
  const items = ref<TokenView[]>([]);
  const loaded = ref(false);
  const loading = ref(false);
  const error = ref<string | null>(null);

  async function fetchList(force = false): Promise<void> {
    if (loaded.value && !force) return;
    loading.value = true;
    error.value = null;
    try {
      items.value = await tokensApi.list();
      loaded.value = true;
    } catch (e) {
      error.value = messageOf(e);
    } finally {
      loading.value = false;
    }
  }

  /**
   * Throws `ApiError` on failure — same contract as `createResourceStore`'s writes, so the form
   * owns its own submit/error state. Returns the full response (plaintext included) to the
   * caller; only the non-secret fields get folded into `items`, so the token itself never enters
   * reactive state that could survive past the reveal screen.
   */
  async function create(body: TokenCreateRequest): Promise<TokenCreateResponse> {
    const created = await tokensApi.create(body);
    const view: TokenView = {
      id: created.id,
      token_prefix: created.token_prefix,
      label: created.label,
      created_at: new Date().toISOString(),
      last_used_at: null,
      expires_at: created.expires_at,
      revoked_at: null,
      endpoints: created.endpoints,
      control_plane: created.control_plane,
    };
    items.value = [view, ...items.value];
    return created;
  }

  async function revoke(id: string): Promise<void> {
    await tokensApi.revoke(id);
    // Revoked tokens stay listed (the whole point of `revoked_at`) — mark rather than remove.
    const revokedAt = new Date().toISOString();
    items.value = items.value.map((t) => (t.id === id ? { ...t, revoked_at: revokedAt } : t));
  }

  return { items, loaded, loading, error, fetchList, create, revoke };
});
