<script setup lang="ts">
// A single run's audit record — the full row `GET /api/runs/{id}` returns, not a trimmed view of
// it. This is where the product's "auditability" claim becomes something a person can look at.
import { computed, onMounted, ref } from "vue";
import { useRunsStore } from "@/stores/runs";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import RunCallTimeline from "@/components/RunCallTimeline.vue";
import RunBudgetUsage from "@/components/RunBudgetUsage.vue";
import RunErrorList from "@/components/RunErrorList.vue";
import JsonViewer from "@/components/JsonViewer.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import StatusPill from "@/components/StatusPill.vue";
import { runStatusTone } from "@/lib/tone";
import { formatBytes, formatDateTime, formatMillis } from "@/lib/format";

const props = defineProps<{ id: string }>();
const store = useRunsStore();

function load(): void {
  store.fetchOne(props.id);
}
onMounted(load);

const run = computed(() => store.detail);
const targetPath = computed(() => {
  const r = run.value;
  if (!r) return "#";
  return r.target_kind === "api_call" ? `/api-calls/${r.target_slug}` : `/scripts/${r.target_slug}`;
});

// The definition snapshot is the thing you go looking for occasionally and never want in your
// way the rest of the time (see this view's brief) — collapsed until asked for, every time.
const showSnapshot = ref(false);
</script>

<template>
  <LoadingState v-if="store.detailLoading" label="loading run" />
  <ErrorState v-else-if="store.detailError" :message="store.detailError" @retry="load" />
  <div v-else-if="run">
    <PageHeader :title="run.tool_name" subtitle="run" :back="{ to: '/runs', label: 'runs' }">
      <template #actions>
        <StatusPill :label="run.status" :tone="runStatusTone(run.status)" />
      </template>
    </PageHeader>

    <DetailSection title="summary">
      <dl>
        <FieldRow label="id" mono>{{ run.id }}</FieldRow>
        <FieldRow label="when">{{ formatDateTime(run.created_at) }}</FieldRow>
        <FieldRow label="execution start">
          {{ formatDateTime(run.execution_start) }}
          <span class="ml-2 text-[11px] text-ink-faint">
            the frozen clock a script's execution_start() reads — replayable from this value only
            if the script never called now() instead
          </span>
        </FieldRow>
        <FieldRow label="endpoint">
          <RouterLink :to="`/endpoints/${run.endpoint_slug}`" class="text-read hover:underline">
            {{ run.endpoint_slug }}
          </RouterLink>
        </FieldRow>
        <FieldRow label="target">
          <RouterLink :to="targetPath" class="text-read hover:underline">{{ run.target_slug }}</RouterLink>
          <span class="ml-2 text-xs text-ink-faint">({{ run.target_kind }})</span>
        </FieldRow>
        <FieldRow label="caller">
          <span class="mr-2 text-[11px] tracking-wide text-ink-faint uppercase">{{ run.caller_kind }}</span>
          <span class="font-mono text-xs text-ink">{{ run.caller_id }}</span>
        </FieldRow>
        <FieldRow label="request id" mono>{{ run.request_id }}</FieldRow>
        <FieldRow label="calls made">{{ run.calls_made }}</FieldRow>
        <FieldRow label="bytes in">{{ formatBytes(run.bytes_in) }}</FieldRow>
        <FieldRow label="pages fetched">{{ run.pages_fetched }}</FieldRow>
        <FieldRow v-if="run.timings" label="elapsed">{{ formatMillis(run.timings.elapsed_ms) }}</FieldRow>
      </dl>
    </DetailSection>

    <DetailSection title="budget usage">
      <RunBudgetUsage :snapshot="run.budget_snapshot" :timings="run.timings" />
    </DetailSection>

    <DetailSection v-if="run.errors && run.errors.length > 0" title="errors">
      <RunErrorList :errors="run.errors" />
    </DetailSection>

    <DetailSection title="definition">
      <dl class="mb-3">
        <FieldRow label="digest" mono>{{ run.definition_digest }}</FieldRow>
      </dl>
      <p class="mb-2 text-xs text-ink-dim">
        The compiled api_call/script/service/budgets slice this run actually executed against.
        Definitions are mutable and last-write-wins, so this snapshot — not the current
        definition — is the only record of what the tool looked like at this moment.
      </p>
      <button
        type="button"
        class="mb-2 border border-border-strong px-2 py-1 text-[10px] tracking-wide text-ink-dim uppercase hover:text-read"
        @click="showSnapshot = !showSnapshot"
      >
        {{ showSnapshot ? "hide" : "show" }} definition snapshot
      </button>
      <JsonViewer v-if="showSnapshot" :value="run.definition_snapshot" />
    </DetailSection>

    <DetailSection title="input">
      <JsonViewer :value="run.input_redacted" null-label="no input recorded" />
    </DetailSection>

    <DetailSection title="result">
      <p class="mb-3 text-xs text-ink-faint">
        Calls are listed by input index (seq), not completion order — this is not a timing trace.
      </p>
      <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <div>
          <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">upstream calls</div>
          <RunCallTimeline :calls="run.calls" />
        </div>
        <div>
          <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">projected output</div>
          <JsonViewer :value="run.output_redacted" null-label="no output recorded" />
        </div>
      </div>
    </DetailSection>
  </div>
</template>
