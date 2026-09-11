<script setup lang="ts">
// The detail view the brief cares most about after runs/plan: it has to make the curation
// visible — the URL template, which params are fixed vs caller-supplied, the projection, and the
// generated `inputSchema` a model actually sees for every endpoint that exposes this call.
import { onMounted } from "vue";
import { useApiCallsStore } from "@/stores/apiCalls";
import { useToolExposure } from "@/composables/useToolExposure";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import ParamTable from "@/components/ParamTable.vue";
import JsonViewer from "@/components/JsonViewer.vue";
import BudgetsSummary from "@/components/BudgetsSummary.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import SeamButton from "@/components/SeamButton.vue";
import StatusPill from "@/components/StatusPill.vue";
import TagChips from "@/components/TagChips.vue";
import { accessTone } from "@/lib/tone";

const props = defineProps<{ slug: string }>();
const store = useApiCallsStore();
const { load: loadExposure, exposures, loading: exposureLoading } = useToolExposure(
  "api_call",
  () => props.slug,
);

function load(): void {
  store.fetchOne(props.slug);
  loadExposure();
}
onMounted(load);

const call = store.bySlug(props.slug);
</script>

<template>
  <LoadingState v-if="store.detailLoading && !call" label="loading api call" />
  <ErrorState v-else-if="store.detailError" :message="store.detailError" @retry="load" />
  <div v-else-if="call">
    <PageHeader :title="call.slug" subtitle="api call" :back="{ to: '/api-calls', label: 'api calls' }">
      <template #actions>
        <StatusPill :label="call.access" :tone="accessTone(call.access)" />
        <SeamButton label="test" />
        <SeamButton label="edit" />
        <SeamButton label="delete" />
      </template>
    </PageHeader>

    <DetailSection title="request">
      <dl>
        <FieldRow label="service">
          <RouterLink :to="`/services/${call.service}`" class="text-read hover:underline">
            {{ call.service }}
          </RouterLink>
        </FieldRow>
        <FieldRow v-if="call.auth_provider" label="auth provider">
          <RouterLink :to="`/auth-providers/${call.auth_provider}`" class="text-read hover:underline">
            {{ call.auth_provider }}
          </RouterLink>
        </FieldRow>
        <FieldRow label="url template" mono>{{ call.method }} {{ call.path_template }}</FieldRow>
        <FieldRow label="idempotent">{{ call.idempotent ? "yes" : "no" }}</FieldRow>
        <FieldRow label="tags"><TagChips :tags="call.tags" /></FieldRow>
        <FieldRow v-if="call.description" label="description">{{ call.description }}</FieldRow>
        <FieldRow v-if="Object.keys(call.query_fixed).length > 0" label="fixed query">
          <span class="font-mono text-xs">
            {{ Object.entries(call.query_fixed).map(([k, v]) => `${k}=${v}`).join(" & ") }}
          </span>
        </FieldRow>
      </dl>
    </DetailSection>

    <DetailSection v-if="call.body_template !== undefined" title="body template">
      <JsonViewer :value="call.body_template" />
    </DetailSection>

    <DetailSection title="parameters">
      <ParamTable :params="call.params" />
    </DetailSection>

    <DetailSection v-if="call.projection" title="projection">
      <p class="mb-3 text-xs text-ink-dim">
        What the model sees is this reshaping of the upstream response, not the raw body.
      </p>
      <div class="overflow-x-auto border border-border">
        <table class="w-full border-collapse text-left text-sm">
          <thead>
            <tr class="border-b border-border bg-surface-raised">
              <th class="px-3 py-2 text-[11px] tracking-[0.1em] text-ink-faint uppercase">name</th>
              <th class="px-3 py-2 text-[11px] tracking-[0.1em] text-ink-faint uppercase">json path</th>
              <th class="px-3 py-2 text-[11px] tracking-[0.1em] text-ink-faint uppercase">cardinality</th>
              <th class="px-3 py-2 text-[11px] tracking-[0.1em] text-ink-faint uppercase">coerce</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="f in call.projection.fields" :key="f.name" class="border-b border-border last:border-b-0">
              <td class="px-3 py-2 font-medium text-ink">{{ f.name }}</td>
              <td class="px-3 py-2 font-mono text-xs text-ink-dim">{{ f.path }}</td>
              <td class="px-3 py-2 text-ink-dim">{{ f.cardinality }}</td>
              <td class="px-3 py-2 text-ink-dim">{{ f.coerce ?? "—" }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </DetailSection>
    <DetailSection v-else title="projection">
      <p class="text-xs text-ink-faint italic">none — the raw upstream response passes through unmodified</p>
    </DetailSection>

    <DetailSection title="exposed as a tool on">
      <LoadingState v-if="exposureLoading && exposures.length === 0" label="resolving endpoint plans" />
      <p v-else-if="exposures.length === 0" class="text-xs text-ink-faint italic">
        no enabled endpoint currently selects this api_call
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
