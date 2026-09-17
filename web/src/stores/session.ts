// `GET /api/me` — who is signed in, shown in the shell header. A failed fetch is left silent:
// a 401 already redirects to /login inside the API client, so by the time this store's `error`
// would matter the page is navigating away.
import { ref } from "vue";
import { defineStore } from "pinia";
import { meApi } from "@/api";
import type { MeView } from "@/api";

export const useSessionStore = defineStore("session", () => {
  const me = ref<MeView | null>(null);

  async function fetch(): Promise<void> {
    try {
      me.value = await meApi.get();
    } catch {
      me.value = null;
    }
  }

  return { me, fetch };
});
