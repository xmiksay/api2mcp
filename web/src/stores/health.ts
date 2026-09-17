import { ref } from "vue";
import { defineStore } from "pinia";
import { ApiError, healthApi } from "@/api";
import type { HealthView } from "@/api";

export const useHealthStore = defineStore("health", () => {
  const data = ref<HealthView | null>(null);
  const loading = ref(false);
  const error = ref<string | null>(null);

  async function fetch(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      data.value = await healthApi.get();
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : "request failed";
    } finally {
      loading.value = false;
    }
  }

  return { data, loading, error, fetch };
});
