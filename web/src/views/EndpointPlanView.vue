<script setup lang="ts">
// `GET /api/endpoints/{slug}/plan` — exactly what `tools/list` returns, plus the statically
// computed reachable-origin set (I2). Along with the runs views, this is one of the two screens
// that shows something a YAML file on its own cannot: everything else in this app is a rendering
// of definitions already in the database.
import { computed, onMounted } from "vue";
import { useEndpointPlanStore } from "@/stores/endpoints";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import BudgetsSummary from "@/components/BudgetsSummary.vue";
import JsonViewer from "@/components/JsonViewer.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import StatusPill from "@/components/StatusPill.vue";
import { accessTone } from "@/lib/tone";

const props = defineProps<{ slug: string }>();
const store = useEndpointPlanStore();
const plan = computed(() => store.plans[props.slug] ?? null);

function load(): void {
  store.fetchPlan(props.slug, true);
}
onMounted(load);
</script>

<template>
  <LoadingState v-if="store.loading && !plan" label="resolving plan" />
  <ErrorState v-else-if="store.error" :message="store.error" @retry="load" />
  <div v-else-if="plan">
    <PageHeader
      :title="`${plan.slug} — plan`"
      subtitle="What POST /mcp/{slug} tools/list actually returns right now."
      :back="{ to: `/endpoints/${props.slug}`, label: 'endpoint' }"
    >
      <template #actions>
        <StatusPill :label="plan.write_ceiling" :tone="accessTone(plan.write_ceiling)" />
      </template>
    </PageHeader>

    <DetailSection title="plan">
      <dl>
        <FieldRow label="definition digest" mono>{{ plan.digest }}</FieldRow>
        <FieldRow label="budgets"><BudgetsSummary :budgets="plan.budgets" /></FieldRow>
        <FieldRow v-if="plan.instructions" label="instructions">{{ plan.instructions }}</FieldRow>
      </dl>
    </DetailSection>

    <DetailSection title="reachable origins (I2)">
      <p class="mb-3 text-xs text-ink-dim">
        Computed statically at resolve time from every selected api_call's service — this is the
        whole reachable blast radius of this endpoint, known before any tool ever runs.
      </p>
      <ul class="flex flex-col gap-1">
        <li
          v-for="origin in plan.origins"
          :key="origin"
          class="border border-border-strong bg-surface-raised px-2 py-1 font-mono text-xs text-ink"
        >
          {{ origin }}
        </li>
      </ul>
      <p v-if="plan.origins.length === 0" class="text-xs text-ink-faint italic">
        no origin is reachable — this endpoint currently exposes no api_call
      </p>
    </DetailSection>

    <DetailSection :title="`tools (${plan.tools.length})`">
      <p v-if="plan.tools.length === 0" class="text-xs text-ink-faint italic">
        this endpoint's tag expression currently selects nothing
      </p>
      <div v-else class="flex flex-col gap-4">
        <div v-for="tool in plan.tools" :key="tool.name" class="border border-border p-3">
          <div class="mb-2 flex flex-wrap items-center gap-3">
            <span class="font-mono text-sm text-ink">{{ tool.name }}</span>
            <StatusPill :label="tool.target_kind" tone="neutral" />
            <RouterLink
              :to="tool.target_kind === 'api_call' ? `/api-calls/${tool.target_slug}` : `/scripts/${tool.target_slug}`"
              class="text-xs text-read hover:underline"
            >
              {{ tool.target_slug }}
            </RouterLink>
          </div>
          <BudgetsSummary :budgets="tool.budgets" />
          <div class="mt-2">
            <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">input schema</div>
            <JsonViewer :value="tool.input_schema" />
          </div>
        </div>
      </div>
    </DetailSection>
  </div>
</template>
