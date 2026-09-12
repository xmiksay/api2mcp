<script setup lang="ts">
// The self-serve counterpart to the CLI's `user add` flow: lets a signed-in user mint their own
// MCP token without shell access. `mode` stays implicit in which of {form, reveal, table} is
// visible — only one of "generating" and "just generated, not yet acknowledged" can be true at a
// time, and the reveal panel owns the plaintext for exactly as long as it's mounted.
import { computed, onMounted, ref } from "vue";
import type { TokenCreateResponse } from "@/api";
import { useTokensStore } from "@/stores/tokens";
import { useEndpointsStore } from "@/stores/endpoints";
import PageHeader from "@/components/PageHeader.vue";
import LoadingState from "@/components/LoadingState.vue";
import EmptyState from "@/components/EmptyState.vue";
import ErrorState from "@/components/ErrorState.vue";
import DetailSection from "@/components/DetailSection.vue";
import TokenGenerateForm from "@/components/tokens/TokenGenerateForm.vue";
import TokenRevealPanel from "@/components/tokens/TokenRevealPanel.vue";
import TokenTable from "@/components/tokens/TokenTable.vue";
import TokenUsageHint from "@/components/tokens/TokenUsageHint.vue";
import { writeButtonClass } from "@/lib/formStyle";

const store = useTokensStore();
const endpoints = useEndpointsStore();
onMounted(() => {
  store.fetchList();
  endpoints.fetchList();
});

const showForm = ref(false);
const revealed = ref<TokenCreateResponse | null>(null);
const allSlugs = computed(() => endpoints.items.map((e) => e.slug));

function onCreated(token: TokenCreateResponse): void {
  showForm.value = false;
  revealed.value = token;
}

// The only place the plaintext's reference is dropped — once this runs, nothing in the app holds
// it anymore (TokenGenerateForm never kept a copy, and the store only ever saw the non-secret view).
function dismissReveal(): void {
  revealed.value = null;
}
</script>

<template>
  <PageHeader title="Access Tokens" subtitle="Self-serve MCP credentials — generate one instead of asking for CLI access.">
    <template #actions>
      <button
        v-if="!showForm && !revealed"
        type="button"
        :class="writeButtonClass"
        @click="showForm = true"
      >
        generate token
      </button>
    </template>
  </PageHeader>

  <TokenRevealPanel v-if="revealed" :token="revealed" :all-slugs="allSlugs" @dismiss="dismissReveal" />
  <TokenGenerateForm v-else-if="showForm" @created="onCreated" @cancel="showForm = false" />

  <DetailSection v-if="!revealed" title="connect a client">
    <p class="mb-3 text-xs text-ink-dim">
      Any live token authenticates this way — point at whichever endpoint(s) it was granted.
    </p>
    <TokenUsageHint :endpoints="[]" :all-slugs="allSlugs" />
  </DetailSection>

  <LoadingState v-if="store.loading" label="loading tokens" />
  <ErrorState v-else-if="store.error" :message="store.error" @retry="() => store.fetchList(true)" />
  <EmptyState
    v-else-if="store.items.length === 0"
    title="no tokens yet"
    hint="Generate one above to let an MCP client authenticate as you."
  />
  <TokenTable v-else :tokens="store.items" />
</template>
