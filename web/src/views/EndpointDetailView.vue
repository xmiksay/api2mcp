<script setup lang="ts">
import { onMounted } from "vue";
import { useEndpointsStore } from "@/stores/endpoints";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import BudgetsSummary from "@/components/BudgetsSummary.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import SeamButton from "@/components/SeamButton.vue";
import StatusPill from "@/components/StatusPill.vue";
import { accessTone } from "@/lib/tone";
import type { PackEndpointTarget } from "@/api";

const props = defineProps<{ slug: string }>();
const store = useEndpointsStore();

function load(): void {
  store.fetchOne(props.slug);
}
onMounted(load);

const endpoint = store.bySlug(props.slug);

function targetLink(t: PackEndpointTarget): { to: string; label: string } {
  if ("api_call" in t) return { to: `/api-calls/${t.api_call}`, label: t.api_call };
  return { to: `/scripts/${t.script}`, label: t.script };
}
</script>

<template>
  <LoadingState v-if="store.detailLoading && !endpoint" label="loading endpoint" />
  <ErrorState v-else-if="store.detailError" :message="store.detailError" @retry="load" />
  <div v-else-if="endpoint">
    <PageHeader :title="endpoint.slug" subtitle="endpoint" :back="{ to: '/endpoints', label: 'endpoints' }">
      <template #actions>
        <RouterLink
          :to="`/endpoints/${endpoint.slug}/plan`"
          class="border border-read/50 px-3 py-1.5 text-xs tracking-wide text-read uppercase hover:bg-read/10"
        >
          view plan
        </RouterLink>
        <SeamButton label="edit" />
        <SeamButton label="delete" />
      </template>
    </PageHeader>

    <DetailSection title="selection">
      <dl>
        <FieldRow label="tag expression" mono>{{ endpoint.tag_expr }}</FieldRow>
        <FieldRow label="write ceiling">
          <StatusPill :label="endpoint.write_ceiling" :tone="accessTone(endpoint.write_ceiling)" />
        </FieldRow>
        <FieldRow label="enabled">
          <StatusPill :label="endpoint.enabled ? 'enabled' : 'disabled'" :tone="endpoint.enabled ? 'ok' : 'neutral'" />
        </FieldRow>
        <FieldRow label="budgets"><BudgetsSummary :budgets="endpoint.budgets" /></FieldRow>
        <FieldRow v-if="endpoint.instructions" label="instructions">{{ endpoint.instructions }}</FieldRow>
        <FieldRow label="scoped auth providers">
          <span v-if="endpoint.auth_providers.length === 0" class="text-xs text-ink-faint italic">
            every provider belonging to a selected service
          </span>
          <span v-else class="flex flex-wrap gap-2">
            <RouterLink
              v-for="p in endpoint.auth_providers"
              :key="p"
              :to="`/auth-providers/${p}`"
              class="font-mono text-xs text-read hover:underline"
            >{{ p }}</RouterLink>
          </span>
        </FieldRow>
      </dl>
    </DetailSection>

    <DetailSection title="aliases">
      <p v-if="Object.keys(endpoint.aliases).length === 0" class="text-xs text-ink-faint italic">
        none — every tool is exposed under its own slug
      </p>
      <ul v-else class="flex flex-col gap-1">
        <li v-for="(target, alias) in endpoint.aliases" :key="alias" class="font-mono text-xs">
          {{ alias }} &rarr;
          <RouterLink :to="targetLink(target).to" class="text-read hover:underline">
            {{ targetLink(target).label }}
          </RouterLink>
        </li>
      </ul>
    </DetailSection>
  </div>
</template>
