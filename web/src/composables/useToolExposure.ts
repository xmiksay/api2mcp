// Derives "which endpoints expose this api_call/script, as what tool" by cross-referencing the
// endpoints list against each endpoint's already-resolved plan. There is no direct API for "which
// endpoints select definition X" — an endpoint's tag_expr is only evaluated server-side inside
// `resolve::build_plan` — so the honest way to answer it client-side is to fetch every endpoint's
// plan and look for a tool whose target matches, not to re-implement tag-expression evaluation
// here. A plain composable rather than a store: it holds no state of its own, only derives from
// `useEndpointsStore`/`useEndpointPlanStore`.
import { computed } from "vue";
import { useEndpointsStore, useEndpointPlanStore } from "@/stores/endpoints";
import type { ToolView } from "@/api";

export interface ToolExposure {
  endpointSlug: string;
  tool: ToolView;
}

export function useToolExposure(targetKind: "api_call" | "script", targetSlug: () => string) {
  const endpoints = useEndpointsStore();
  const plans = useEndpointPlanStore();

  async function load(): Promise<void> {
    await endpoints.fetchList();
    await Promise.all(endpoints.items.map((ep) => plans.fetchPlan(ep.slug)));
  }

  const exposures = computed<ToolExposure[]>(() => {
    const slug = targetSlug();
    const result: ToolExposure[] = [];
    for (const ep of endpoints.items) {
      const plan = plans.plans[ep.slug];
      if (!plan) continue;
      for (const tool of plan.tools) {
        if (tool.target_kind === targetKind && tool.target_slug === slug) {
          result.push({ endpointSlug: ep.slug, tool });
        }
      }
    }
    return result;
  });

  return { load, exposures, loading: computed(() => endpoints.loading || plans.loading) };
}
