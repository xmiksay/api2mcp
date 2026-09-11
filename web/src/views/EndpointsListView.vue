<script setup lang="ts">
import { onMounted } from "vue";
import { useRouter } from "vue-router";
import { useEndpointsStore } from "@/stores/endpoints";
import PageHeader from "@/components/PageHeader.vue";
import DataTable from "@/components/DataTable.vue";
import LoadingState from "@/components/LoadingState.vue";
import EmptyState from "@/components/EmptyState.vue";
import ErrorState from "@/components/ErrorState.vue";
import StatusPill from "@/components/StatusPill.vue";
import { accessTone } from "@/lib/tone";
import type { Column } from "@/lib/table";
import type { EndpointView } from "@/api";
import { writeButtonClass } from "@/lib/formStyle";

const store = useEndpointsStore();
const router = useRouter();
onMounted(() => store.fetchList());

const columns: Column[] = [
  { key: "slug", label: "slug" },
  { key: "tag_expr", label: "tag expression" },
  { key: "write_ceiling", label: "write ceiling" },
  { key: "enabled", label: "enabled" },
];

function open(row: EndpointView): void {
  router.push(`/endpoints/${row.slug}`);
}
</script>

<template>
  <PageHeader
    title="Endpoints"
    subtitle="MCP-shaped views over the definitions above — POST /mcp/{slug} serves exactly what a plan resolves."
  >
    <template #actions>
      <RouterLink to="/endpoints/new" :class="writeButtonClass">new endpoint</RouterLink>
    </template>
  </PageHeader>

  <LoadingState v-if="store.loading" label="loading endpoints" />
  <ErrorState v-else-if="store.error" :message="store.error" @retry="() => store.fetchList(true)" />
  <EmptyState
    v-else-if="store.items.length === 0"
    title="no endpoints defined"
    hint="An endpoint selects a subset of api_calls/scripts by tag expression and exposes them at /mcp/{slug}."
  >
    <RouterLink to="/endpoints/new" :class="writeButtonClass">new endpoint</RouterLink>
  </EmptyState>
  <DataTable v-else :columns="columns" :rows="store.items" :row-key="(r) => r.slug" @row-click="open">
    <template #cell-slug="{ row }">
      <span class="font-medium text-ink">{{ row.slug }}</span>
    </template>
    <template #cell-tag_expr="{ row }">
      <span class="font-mono text-xs text-ink-dim">{{ row.tag_expr }}</span>
    </template>
    <template #cell-write_ceiling="{ row }">
      <StatusPill :label="row.write_ceiling" :tone="accessTone(row.write_ceiling)" />
    </template>
    <template #cell-enabled="{ row }">
      <StatusPill :label="row.enabled ? 'enabled' : 'disabled'" :tone="row.enabled ? 'ok' : 'neutral'" />
    </template>
  </DataTable>
</template>
