// Endpoints get one extra concern the other four definition stores don't: the resolved plan
// (`GET /api/endpoints/{slug}/plan`) is a separate, per-slug cached fetch, not part of `EndpointView`.
import { ref } from "vue";
import { defineStore } from "pinia";
import { ApiError, endpointsApi } from "@/api";
import type { PlanView } from "@/api";
import { createResourceStore } from "./createResourceStore";

export const useEndpointsStore = createResourceStore("endpoints", endpointsApi);

export const useEndpointPlanStore = defineStore("endpointPlan", () => {
  const plans = ref<Record<string, PlanView>>({});
  const loading = ref(false);
  const error = ref<string | null>(null);

  async function fetchPlan(slug: string, force = false): Promise<void> {
    if (plans.value[slug] && !force) return;
    loading.value = true;
    error.value = null;
    try {
      plans.value[slug] = await endpointsApi.plan(slug);
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : "request failed";
    } finally {
      loading.value = false;
    }
  }

  return { plans, loading, error, fetchPlan };
});
