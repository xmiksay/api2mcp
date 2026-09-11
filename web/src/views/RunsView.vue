<script setup lang="ts">
// The run log. `RunsView`/`RunDetailView` are — together with `EndpointPlanView` — the only
// screens showing something a pack.yaml can't: what actually happened, not what was declared.
import { onMounted } from "vue";
import { useRouter } from "vue-router";
import { useRunsStore } from "@/stores/runs";
import { useEndpointsStore } from "@/stores/endpoints";
import PageHeader from "@/components/PageHeader.vue";
import DataTable from "@/components/DataTable.vue";
import LoadingState from "@/components/LoadingState.vue";
import EmptyState from "@/components/EmptyState.vue";
import ErrorState from "@/components/ErrorState.vue";
import StatusPill from "@/components/StatusPill.vue";
import { runStatusTone } from "@/lib/tone";
import { formatBytes, formatDateTime } from "@/lib/format";
import type { Column } from "@/lib/table";
import type { RunStatus, RunSummaryView } from "@/api";

const store = useRunsStore();
const endpoints = useEndpointsStore();
const router = useRouter();

onMounted(() => {
  endpoints.fetchList();
  store.resetAndFetch();
});

const STATUSES: RunStatus[] = ["ok", "partial", "error", "denied", "budget_exceeded", "timeout"];

const columns: Column[] = [
  { key: "created_at", label: "when" },
  { key: "endpoint_slug", label: "endpoint" },
  { key: "tool_name", label: "tool" },
  { key: "status", label: "status" },
  { key: "calls_made", label: "calls", class: "text-right" },
  { key: "bytes_in", label: "bytes in", class: "text-right" },
  { key: "pages_fetched", label: "pages", class: "text-right" },
];

function open(row: RunSummaryView): void {
  router.push(`/runs/${row.id}`);
}
</script>

<template>
  <PageHeader title="Runs" subtitle="Every tool invocation, most recent first.">
    <template #actions>
      <select
        v-model="store.filter.endpoint"
        class="border border-border-strong bg-surface px-2 py-1.5 text-xs text-ink"
        @change="store.resetAndFetch"
      >
        <option value="">all endpoints</option>
        <option v-for="ep in endpoints.items" :key="ep.slug" :value="ep.slug">{{ ep.slug }}</option>
      </select>
      <select
        v-model="store.filter.status"
        class="border border-border-strong bg-surface px-2 py-1.5 text-xs text-ink"
        @change="store.resetAndFetch"
      >
        <option value="">any status</option>
        <option v-for="s in STATUSES" :key="s" :value="s">{{ s }}</option>
      </select>
    </template>
  </PageHeader>

  <LoadingState v-if="store.loading" label="loading runs" />
  <ErrorState v-else-if="store.error" :message="store.error" @retry="store.fetchList" />
  <EmptyState
    v-else-if="store.items.length === 0"
    title="no runs recorded yet"
    hint="Call a tool through /mcp/{endpoint} — or the test panel, once it exists — and it will show up here."
  />
  <template v-else>
    <DataTable :columns="columns" :rows="store.items" :row-key="(r) => r.id" @row-click="open">
      <template #cell-created_at="{ row }">
        <span class="font-mono text-xs text-ink-dim">{{ formatDateTime(row.created_at) }}</span>
      </template>
      <template #cell-endpoint_slug="{ row }">
        <RouterLink :to="`/endpoints/${row.endpoint_slug}`" class="text-read hover:underline" @click.stop>
          {{ row.endpoint_slug }}
        </RouterLink>
      </template>
      <template #cell-tool_name="{ row }">
        <span class="font-mono text-xs text-ink">{{ row.tool_name }}</span>
      </template>
      <template #cell-status="{ row }">
        <StatusPill :label="row.status" :tone="runStatusTone(row.status)" />
      </template>
      <template #cell-bytes_in="{ row }">
        <span class="text-ink-dim">{{ formatBytes(row.bytes_in) }}</span>
      </template>
    </DataTable>

    <div class="mt-3 flex items-center justify-between text-xs text-ink-faint">
      <span>offset {{ store.filter.offset }}</span>
      <div class="flex gap-2">
        <button
          type="button"
          class="border border-border-strong px-2 py-1 uppercase disabled:opacity-40"
          :disabled="store.filter.offset === 0"
          @click="store.prevPage"
        >
          prev
        </button>
        <button
          type="button"
          class="border border-border-strong px-2 py-1 uppercase disabled:opacity-40"
          :disabled="!store.hasMore"
          @click="store.nextPage"
        >
          next
        </button>
      </div>
    </div>
  </template>
</template>
