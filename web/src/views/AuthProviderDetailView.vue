<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { useAuthProvidersStore } from "@/stores/authProviders";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import ConfirmButton from "@/components/form/ConfirmButton.vue";
import ValidationErrors from "@/components/form/ValidationErrors.vue";
import { ghostButtonClass } from "@/lib/formStyle";
import { ApiError } from "@/api";

const props = defineProps<{ slug: string }>();
const store = useAuthProvidersStore();
const router = useRouter();

function load(): void {
  store.fetchOne(props.slug);
}
onMounted(load);

const provider = store.bySlug(props.slug);

const deleting = ref(false);
const deleteErrors = ref<string[]>([]);

async function remove(): Promise<void> {
  deleting.value = true;
  deleteErrors.value = [];
  try {
    await store.remove(props.slug);
    router.push("/auth-providers");
  } catch (e) {
    deleteErrors.value = e instanceof ApiError ? e.messages : ["request failed"];
  } finally {
    deleting.value = false;
  }
}
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
        <RouterLink :to="`/auth-providers/${provider.slug}/edit`" :class="ghostButtonClass">edit</RouterLink>
        <ConfirmButton label="delete" :pending="deleting" @confirm="remove" />
      </template>
    </PageHeader>

    <ValidationErrors :errors="deleteErrors" />

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
