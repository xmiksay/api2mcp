// The run log. Unlike the definition stores, the list is filtered/paged server-side
// (`RunFilter` in `src/store/run.rs`) rather than fetched whole and cached — a growing audit
// trail is exactly the case a full-list cache doesn't scale to.
import { reactive, ref } from "vue";
import { defineStore } from "pinia";
import { ApiError, runsApi } from "@/api";
import type { RunDetailView, RunFilterParams, RunStatus, RunSummaryView } from "@/api";

const PAGE_SIZE = 50;

function messageOf(e: unknown): string {
  return e instanceof ApiError ? e.message : "request failed";
}

export const useRunsStore = defineStore("runs", () => {
  const items = ref<RunSummaryView[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const filter = reactive<{ endpoint: string; status: RunStatus | ""; offset: number }>({
    endpoint: "",
    status: "",
    offset: 0,
  });
  /** One past the end of the current page implies more; the API returns no total count. */
  const hasMore = ref(false);

  async function fetchList(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      const params: RunFilterParams = {
        limit: PAGE_SIZE + 1,
        offset: filter.offset,
      };
      if (filter.endpoint) params.endpoint = filter.endpoint;
      if (filter.status) params.status = filter.status;
      const rows = await runsApi.list(params);
      hasMore.value = rows.length > PAGE_SIZE;
      items.value = rows.slice(0, PAGE_SIZE);
    } catch (e) {
      error.value = messageOf(e);
    } finally {
      loading.value = false;
    }
  }

  function resetAndFetch(): Promise<void> {
    filter.offset = 0;
    return fetchList();
  }

  function nextPage(): Promise<void> {
    filter.offset += PAGE_SIZE;
    return fetchList();
  }

  function prevPage(): Promise<void> {
    filter.offset = Math.max(0, filter.offset - PAGE_SIZE);
    return fetchList();
  }

  const detail = ref<RunDetailView | null>(null);
  const detailLoading = ref(false);
  const detailError = ref<string | null>(null);

  async function fetchOne(id: string): Promise<void> {
    detailLoading.value = true;
    detailError.value = null;
    detail.value = null;
    try {
      detail.value = await runsApi.get(id);
    } catch (e) {
      detailError.value = messageOf(e);
    } finally {
      detailLoading.value = false;
    }
  }

  return {
    items,
    loading,
    error,
    filter,
    hasMore,
    pageSize: PAGE_SIZE,
    fetchList,
    resetAndFetch,
    nextPage,
    prevPage,
    detail,
    detailLoading,
    detailError,
    fetchOne,
  };
});
