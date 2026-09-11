// `GET /api/tags` — the flat tag vocabulary. Read-only and small enough that a plain list cache
// (no per-slug detail, tags have no detail view) is all this needs.
import { ref } from "vue";
import { defineStore } from "pinia";
import { ApiError, tagsApi } from "@/api";

export const useTagsStore = defineStore("tags", () => {
  const items = ref<string[]>([]);
  const loaded = ref(false);
  const loading = ref(false);
  const error = ref<string | null>(null);

  async function fetchList(force = false): Promise<void> {
    if (loaded.value && !force) return;
    loading.value = true;
    error.value = null;
    try {
      items.value = await tagsApi.list();
      loaded.value = true;
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : "request failed";
    } finally {
      loading.value = false;
    }
  }

  return { items, loaded, loading, error, fetchList };
});
