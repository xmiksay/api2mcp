<script setup lang="ts">
import { onMounted } from "vue";
import { useRouter } from "vue-router";
import { useServicesStore } from "@/stores/services";
import PageHeader from "@/components/PageHeader.vue";
import DataTable from "@/components/DataTable.vue";
import LoadingState from "@/components/LoadingState.vue";
import EmptyState from "@/components/EmptyState.vue";
import ErrorState from "@/components/ErrorState.vue";
import SeamButton from "@/components/SeamButton.vue";
import type { Column } from "@/lib/table";
import type { ServiceView } from "@/api";

const store = useServicesStore();
const router = useRouter();
onMounted(() => store.fetchList());

const columns: Column[] = [
  { key: "slug", label: "slug" },
  { key: "base_url", label: "base url" },
  { key: "origin_allowlist", label: "allowlist" },
  { key: "max_concurrency", label: "concurrency", class: "text-right" },
  { key: "max_response_bytes", label: "max bytes", class: "text-right" },
];

function open(row: ServiceView): void {
  router.push(`/services/${row.slug}`);
}
</script>

<template>
  <PageHeader title="Services" subtitle="Upstream APIs — base URL, allowlisted origins, transport limits.">
    <template #actions>
      <SeamButton label="new service" />
    </template>
  </PageHeader>

  <LoadingState v-if="store.loading" label="loading services" />
  <ErrorState v-else-if="store.error" :message="store.error" @retry="() => store.fetchList(true)" />
  <EmptyState
    v-else-if="store.items.length === 0"
    title="no services defined"
    hint="Services are the upstream APIs an api_call points at. Import a pack or add one to get started."
  >
    <SeamButton label="new service" />
  </EmptyState>
  <DataTable
    v-else
    :columns="columns"
    :rows="store.items"
    :row-key="(r) => r.slug"
    @row-click="open"
  >
    <template #cell-slug="{ row }">
      <span class="font-medium text-ink">{{ row.slug }}</span>
    </template>
    <template #cell-base_url="{ row }">
      <span class="text-ink-dim">{{ row.base_url }}</span>
    </template>
    <template #cell-origin_allowlist="{ row }">
      <span class="text-xs text-ink-faint">{{ row.origin_allowlist.length }} origin(s)</span>
    </template>
  </DataTable>
</template>
