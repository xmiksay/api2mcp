<script setup lang="ts">
import { onMounted } from "vue";
import { useRouter } from "vue-router";
import { useApiCallsStore } from "@/stores/apiCalls";
import PageHeader from "@/components/PageHeader.vue";
import DataTable from "@/components/DataTable.vue";
import LoadingState from "@/components/LoadingState.vue";
import EmptyState from "@/components/EmptyState.vue";
import ErrorState from "@/components/ErrorState.vue";
import StatusPill from "@/components/StatusPill.vue";
import TagChips from "@/components/TagChips.vue";
import { accessTone } from "@/lib/tone";
import type { Column } from "@/lib/table";
import type { ApiCallView } from "@/api";
import { writeButtonClass } from "@/lib/formStyle";

const store = useApiCallsStore();
const router = useRouter();
onMounted(() => store.fetchList());

const columns: Column[] = [
  { key: "slug", label: "slug" },
  { key: "service", label: "service" },
  { key: "method", label: "method" },
  { key: "path_template", label: "path" },
  { key: "access", label: "access" },
  { key: "tags", label: "tags" },
];

function open(row: ApiCallView): void {
  router.push(`/api-calls/${row.slug}`);
}
</script>

<template>
  <PageHeader title="Api Calls" subtitle="Curated HTTP calls — one exact request shape each, ready to become a tool.">
    <template #actions>
      <RouterLink to="/api-calls/new" :class="writeButtonClass">new api call</RouterLink>
    </template>
  </PageHeader>

  <LoadingState v-if="store.loading" label="loading api calls" />
  <ErrorState v-else-if="store.error" :message="store.error" @retry="() => store.fetchList(true)" />
  <EmptyState
    v-else-if="store.items.length === 0"
    title="no api calls defined"
    hint="An api_call is one HTTP request shape — method, URL template, params and a projection."
  >
    <RouterLink to="/api-calls/new" :class="writeButtonClass">new api call</RouterLink>
  </EmptyState>
  <DataTable v-else :columns="columns" :rows="store.items" :row-key="(r) => r.slug" @row-click="open">
    <template #cell-slug="{ row }">
      <span class="font-medium text-ink">{{ row.slug }}</span>
    </template>
    <template #cell-service="{ row }">
      <RouterLink :to="`/services/${row.service}`" class="text-read hover:underline" @click.stop>
        {{ row.service }}
      </RouterLink>
    </template>
    <template #cell-method="{ row }">
      <span class="font-mono text-xs text-ink-dim">{{ row.method }}</span>
    </template>
    <template #cell-path_template="{ row }">
      <span class="font-mono text-xs text-ink-dim">{{ row.path_template }}</span>
    </template>
    <template #cell-access="{ row }">
      <StatusPill :label="row.access" :tone="accessTone(row.access)" />
    </template>
    <template #cell-tags="{ row }">
      <TagChips :tags="row.tags" />
    </template>
  </DataTable>
</template>
