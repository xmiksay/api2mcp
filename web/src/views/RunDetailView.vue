<script setup lang="ts">
// A single run's audit record. `GET /api/runs/{id}` currently returns the summary plus the
// per-call timeline — see this file's own note below for the richer fields (definition snapshot,
// redacted input, the errors array, budget usage, per-call method/URL/duration) that exist on the
// `runs`/`run_calls` tables and in `store::run` but aren't wired onto this route yet.
import { computed, onMounted } from "vue";
import { useRunsStore } from "@/stores/runs";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import RunCallTimeline from "@/components/RunCallTimeline.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import StatusPill from "@/components/StatusPill.vue";
import { runStatusTone } from "@/lib/tone";
import { formatBytes, formatDateTime } from "@/lib/format";

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
        <FieldRow label="endpoint">
          <RouterLink :to="`/endpoints/${run.endpoint_slug}`" class="text-read hover:underline">
            {{ run.endpoint_slug }}
          </RouterLink>
        </FieldRow>
        <FieldRow label="target">
          <RouterLink :to="targetPath" class="text-read hover:underline">{{ run.target_slug }}</RouterLink>
          <span class="ml-2 text-xs text-ink-faint">({{ run.target_kind }})</span>
        </FieldRow>
        <FieldRow label="calls made">{{ run.calls_made }}</FieldRow>
        <FieldRow label="bytes in">{{ formatBytes(run.bytes_in) }}</FieldRow>
        <FieldRow label="pages fetched">{{ run.pages_fetched }}</FieldRow>
      </dl>
    </DetailSection>

    <DetailSection title="upstream call timeline">
      <RunCallTimeline :calls="run.calls" />
    </DetailSection>

    <p class="mt-2 text-xs text-ink-faint italic">
      The definition snapshot, redacted input/output, budget usage and the errors array are
      recorded on this run (`runs`/`run_calls`) but `GET /api/runs/{id}` does not serialize them
      yet — this view renders everything the route currently exposes.
    </p>
  </div>
</template>
