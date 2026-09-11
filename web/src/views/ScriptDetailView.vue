<script setup lang="ts">
import { onMounted } from "vue";
import { useScriptsStore } from "@/stores/scripts";
import { useToolExposure } from "@/composables/useToolExposure";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import ParamTable from "@/components/ParamTable.vue";
import CodeBlock from "@/components/CodeBlock.vue";
import JsonViewer from "@/components/JsonViewer.vue";
import BudgetsSummary from "@/components/BudgetsSummary.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import SeamButton from "@/components/SeamButton.vue";
import TagChips from "@/components/TagChips.vue";

const props = defineProps<{ slug: string }>();
const store = useScriptsStore();
const { load: loadExposure, exposures, loading: exposureLoading } = useToolExposure(
  "script",
  () => props.slug,
);

function load(): void {
  store.fetchOne(props.slug);
  loadExposure();
}
onMounted(load);

const script = store.bySlug(props.slug);
</script>

<template>
  <LoadingState v-if="store.detailLoading && !script" label="loading script" />
  <ErrorState v-else-if="store.detailError" :message="store.detailError" @retry="load" />
  <div v-else-if="script">
    <PageHeader :title="script.slug" subtitle="script" :back="{ to: '/scripts', label: 'scripts' }">
      <template #actions>
        <SeamButton label="test" />
        <SeamButton label="edit" />
        <SeamButton label="delete" />
      </template>
    </PageHeader>

    <DetailSection title="declared">
      <dl>
        <FieldRow label="tags"><TagChips :tags="script.tags" /></FieldRow>
        <FieldRow v-if="script.description" label="description">{{ script.description }}</FieldRow>
        <FieldRow label="own budget"><BudgetsSummary :budgets="script.budgets" /></FieldRow>
        <FieldRow label="callable api_calls (I1)">
          <ul class="flex flex-col gap-0.5">
            <li v-for="(target, alias) in script.callable" :key="alias" class="font-mono text-xs">
              {{ alias }} &rarr;
              <RouterLink :to="`/api-calls/${target}`" class="text-read hover:underline">{{ target }}</RouterLink>
            </li>
          </ul>
          <p class="mt-1 text-xs text-ink-faint">
            This is the full set of HTTP calls this script can ever make — `api()`/`api_many()` in
            the source below can only address these aliases, never an arbitrary URL.
          </p>
        </FieldRow>
      </dl>
    </DetailSection>

    <DetailSection title="parameters">
      <ParamTable :params="script.params" />
    </DetailSection>

    <DetailSection title="source">
      <CodeBlock :code="script.source" />
    </DetailSection>

    <DetailSection title="exposed as a tool on">
      <LoadingState v-if="exposureLoading && exposures.length === 0" label="resolving endpoint plans" />
      <p v-else-if="exposures.length === 0" class="text-xs text-ink-faint italic">
        no enabled endpoint currently selects this script
      </p>
      <div v-else class="flex flex-col gap-4">
        <div v-for="exp in exposures" :key="exp.endpointSlug + exp.tool.name" class="border border-border p-3">
          <div class="mb-2 flex items-center gap-3">
            <RouterLink :to="`/endpoints/${exp.endpointSlug}/plan`" class="text-sm text-read hover:underline">
              {{ exp.endpointSlug }}
            </RouterLink>
            <span class="text-xs text-ink-faint">tool name</span>
            <span class="font-mono text-xs text-ink">{{ exp.tool.name }}</span>
          </div>
          <BudgetsSummary :budgets="exp.tool.budgets" />
          <div class="mt-2">
            <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">generated input schema</div>
            <JsonViewer :value="exp.tool.input_schema" />
          </div>
        </div>
      </div>
    </DetailSection>
  </div>
</template>
