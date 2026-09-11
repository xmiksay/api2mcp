<script setup lang="ts">
import { onMounted } from "vue";
import { useAuthProvidersStore } from "@/stores/authProviders";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import SeamButton from "@/components/SeamButton.vue";

const props = defineProps<{ slug: string }>();
const store = useAuthProvidersStore();

function load(): void {
  store.fetchOne(props.slug);
}
onMounted(load);

const provider = store.bySlug(props.slug);
</script>

<template>
  <LoadingState v-if="store.detailLoading && !provider" label="loading auth provider" />
  <ErrorState v-else-if="store.detailError" :message="store.detailError" @retry="load" />
  <div v-else-if="provider">
    <PageHeader
      :title="provider.slug"
      subtitle="auth provider"
      :back="{ to: '/auth-providers', label: 'auth providers' }"
    >
      <template #actions>
        <SeamButton label="edit" />
        <SeamButton label="delete" />
      </template>
    </PageHeader>

    <DetailSection title="binding">
      <dl>
        <FieldRow label="service">
          <RouterLink :to="`/services/${provider.service}`" class="text-read hover:underline">
            {{ provider.service }}
          </RouterLink>
        </FieldRow>
        <FieldRow label="kind">{{ provider.kind }}</FieldRow>
        <FieldRow label="bound origin" mono>{{ provider.bound_origin }}</FieldRow>
        <FieldRow label="scopes">
          <span v-if="provider.scopes.length === 0" class="text-ink-faint italic">none</span>
          <span v-else class="font-mono text-xs">{{ provider.scopes.join(", ") }}</span>
        </FieldRow>
        <FieldRow v-if="provider.token_url" label="token url" mono>{{ provider.token_url }}</FieldRow>
      </dl>
    </DetailSection>

    <DetailSection title="header">
      <dl>
        <FieldRow label="header name" mono>{{ provider.header_name }}</FieldRow>
        <FieldRow label="value template" mono>{{ provider.value_template }}</FieldRow>
        <FieldRow label="credential env key" mono>{{ provider.credential_env_key }}</FieldRow>
      </dl>
      <p class="mt-3 text-xs text-ink-faint">
        Only the env var <em>name</em> above is stored — the credential value itself never enters
        this database, and never reaches this screen (I4).
      </p>
    </DetailSection>
  </div>
</template>
