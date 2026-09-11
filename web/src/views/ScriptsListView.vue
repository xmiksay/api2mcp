<script setup lang="ts">
import { onMounted } from "vue";
import { useRouter } from "vue-router";
import { useScriptsStore } from "@/stores/scripts";
import PageHeader from "@/components/PageHeader.vue";
import DataTable from "@/components/DataTable.vue";
import LoadingState from "@/components/LoadingState.vue";
import EmptyState from "@/components/EmptyState.vue";
import ErrorState from "@/components/ErrorState.vue";
import TagChips from "@/components/TagChips.vue";
import type { Column } from "@/lib/table";
import type { ScriptView } from "@/api";
import { writeButtonClass } from "@/lib/formStyle";

const store = useScriptsStore();
const router = useRouter();
onMounted(() => store.fetchList());

const columns: Column[] = [
  { key: "slug", label: "slug" },
  { key: "callable", label: "calls" },
  { key: "budgets", label: "max calls", class: "text-right" },
  { key: "tags", label: "tags" },
];

function open(row: ScriptView): void {
  router.push(`/scripts/${row.slug}`);
}
</script>

<template>
  <PageHeader
    title="Scripts"
    subtitle="Rhai compositions of declared api_calls — never raw HTTP (I1)."
  >
    <template #actions>
      <RouterLink to="/scripts/new" :class="writeButtonClass">new script</RouterLink>
    </template>
  </PageHeader>

  <LoadingState v-if="store.loading" label="loading scripts" />
  <ErrorState v-else-if="store.error" :message="store.error" @retry="() => store.fetchList(true)" />
  <EmptyState
    v-else-if="store.items.length === 0"
    title="no scripts defined"
    hint="A script folds several api_calls into one model-usable answer."
  >
    <RouterLink to="/scripts/new" :class="writeButtonClass">new script</RouterLink>
  </EmptyState>
  <DataTable v-else :columns="columns" :rows="store.items" :row-key="(r) => r.slug" @row-click="open">
    <template #cell-slug="{ row }">
      <span class="font-medium text-ink">{{ row.slug }}</span>
    </template>
    <template #cell-callable="{ row }">
      <span class="font-mono text-xs text-ink-dim">{{ Object.keys(row.callable).join(", ") || "—" }}</span>
    </template>
    <template #cell-budgets="{ row }">
      <span class="text-ink-dim">{{ row.budgets.max_calls ?? "—" }}</span>
    </template>
    <template #cell-tags="{ row }">
      <TagChips :tags="row.tags" />
    </template>
  </DataTable>
</template>
