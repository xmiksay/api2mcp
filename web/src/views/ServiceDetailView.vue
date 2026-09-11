<script setup lang="ts">
import { computed, onMounted } from "vue";
import { useServicesStore } from "@/stores/services";
import { useApiCallsStore } from "@/stores/apiCalls";
import { useAuthProvidersStore } from "@/stores/authProviders";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import SeamButton from "@/components/SeamButton.vue";
import { formatBytes } from "@/lib/format";

const props = defineProps<{ slug: string }>();

const store = useServicesStore();
const apiCalls = useApiCallsStore();
const authProviders = useAuthProvidersStore();

function load(): void {
  store.fetchOne(props.slug);
  apiCalls.fetchList();
  authProviders.fetchList();
}
onMounted(load);

const service = store.bySlug(props.slug);
const relatedApiCalls = computed(() => apiCalls.items.filter((c) => c.service === props.slug));
const relatedAuthProviders = computed(() => authProviders.items.filter((p) => p.service === props.slug));
</script>

<template>
  <LoadingState v-if="store.detailLoading && !service" label="loading service" />
  <ErrorState v-else-if="store.detailError" :message="store.detailError" @retry="load" />
  <div v-else-if="service">
    <PageHeader :title="service.slug" subtitle="service" :back="{ to: '/services', label: 'services' }">
      <template #actions>
        <SeamButton label="edit" />
        <SeamButton label="delete" />
      </template>
    </PageHeader>

    <DetailSection title="transport">
      <dl>
        <FieldRow label="base url" mono>{{ service.base_url }}</FieldRow>
        <FieldRow label="origin allowlist">
          <ul class="flex flex-col gap-0.5 font-mono text-xs">
            <li v-for="origin in service.origin_allowlist" :key="origin">{{ origin }}</li>
          </ul>
        </FieldRow>
        <FieldRow label="timeout">{{ service.timeout_ms }} ms</FieldRow>
        <FieldRow label="max concurrency">{{ service.max_concurrency }}</FieldRow>
        <FieldRow label="rate limit">
          {{ service.rate_limit_per_min ? `${service.rate_limit_per_min}/min` : "unlimited" }}
        </FieldRow>
        <FieldRow label="max response bytes">{{ formatBytes(service.max_response_bytes) }}</FieldRow>
      </dl>
    </DetailSection>

    <DetailSection v-if="Object.keys(service.default_headers).length > 0" title="default headers">
      <dl>
        <FieldRow v-for="(value, name) in service.default_headers" :key="name" :label="String(name)" mono>
          {{ value }}
        </FieldRow>
      </dl>
    </DetailSection>

    <DetailSection title="auth providers on this service">
      <p v-if="relatedAuthProviders.length === 0" class="text-xs text-ink-faint italic">none</p>
      <ul v-else class="flex flex-col gap-1">
        <li v-for="p in relatedAuthProviders" :key="p.slug">
          <RouterLink :to="`/auth-providers/${p.slug}`" class="text-sm text-read hover:underline">
            {{ p.slug }}
          </RouterLink>
        </li>
      </ul>
    </DetailSection>

    <DetailSection title="api calls on this service">
      <p v-if="relatedApiCalls.length === 0" class="text-xs text-ink-faint italic">none</p>
      <ul v-else class="flex flex-col gap-1">
        <li v-for="c in relatedApiCalls" :key="c.slug">
          <RouterLink :to="`/api-calls/${c.slug}`" class="text-sm text-read hover:underline">
            {{ c.slug }}
          </RouterLink>
          <span class="ml-2 text-xs text-ink-faint">{{ c.method }} {{ c.path_template }}</span>
        </li>
      </ul>
    </DetailSection>
  </div>
</template>
