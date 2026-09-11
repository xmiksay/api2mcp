<script setup lang="ts">
import { onMounted } from "vue";
import { useRouter } from "vue-router";
import { useAuthProvidersStore } from "@/stores/authProviders";
import PageHeader from "@/components/PageHeader.vue";
import DataTable from "@/components/DataTable.vue";
import LoadingState from "@/components/LoadingState.vue";
import EmptyState from "@/components/EmptyState.vue";
import ErrorState from "@/components/ErrorState.vue";
import type { Column } from "@/lib/table";
import type { AuthProviderView } from "@/api";
import { writeButtonClass } from "@/lib/formStyle";

const store = useAuthProvidersStore();
const router = useRouter();
onMounted(() => store.fetchList());

const columns: Column[] = [
  { key: "slug", label: "slug" },
  { key: "service", label: "service" },
  { key: "kind", label: "kind" },
  { key: "credential_env_key", label: "credential env key" },
  { key: "bound_origin", label: "bound origin" },
];

function open(row: AuthProviderView): void {
  router.push(`/auth-providers/${row.slug}`);
}
</script>

<template>
  <PageHeader
    title="Auth Providers"
    subtitle="Credential wiring — an env var name and a bound origin, never a secret value. Human-only writes (I5)."
  >
    <template #actions>
      <RouterLink to="/auth-providers/new" :class="writeButtonClass">new provider</RouterLink>
    </template>
  </PageHeader>

  <LoadingState v-if="store.loading" label="loading auth providers" />
  <ErrorState v-else-if="store.error" :message="store.error" @retry="() => store.fetchList(true)" />
  <EmptyState
    v-else-if="store.items.length === 0"
    title="no auth providers defined"
    hint="Most demo/public APIs need none — this is only for credentialed upstreams."
  >
    <RouterLink to="/auth-providers/new" :class="writeButtonClass">new provider</RouterLink>
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
    <template #cell-credential_env_key="{ row }">
      <span class="font-mono text-xs text-ink-dim">{{ row.credential_env_key }}</span>
    </template>
    <template #cell-bound_origin="{ row }">
      <span class="font-mono text-xs text-ink-dim">{{ row.bound_origin }}</span>
    </template>
  </DataTable>
</template>
