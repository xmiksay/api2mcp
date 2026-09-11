<script setup lang="ts">
// New/edit form for a `PackEndpoint` — the tag-expression selection an MCP client actually sees
// at `/mcp/{slug}`.
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { ApiError } from "@/api";
import type { PackEndpoint } from "@/api";
import { useEndpointsStore } from "@/stores/endpoints";
import { useApiCallsStore } from "@/stores/apiCalls";
import { useScriptsStore } from "@/stores/scripts";
import { useAuthProvidersStore } from "@/stores/authProviders";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import LoadingState from "@/components/LoadingState.vue";
import FormField from "@/components/form/FormField.vue";
import ValidationErrors from "@/components/form/ValidationErrors.vue";
import BudgetsEditor from "@/components/form/BudgetsEditor.vue";
import AliasEditor from "@/components/form/AliasEditor.vue";
import { checkboxClass, fieldClass, ghostButtonClass, primaryButtonClass, textareaClass } from "@/lib/formStyle";
import { slugError } from "@/lib/slug";

const props = defineProps<{ mode: "create" | "edit"; slug?: string }>();
const router = useRouter();
const store = useEndpointsStore();
const apiCalls = useApiCallsStore();
const scripts = useScriptsStore();
const authProviders = useAuthProvidersStore();

function blank(): PackEndpoint {
  return { tag_expr: "", write_ceiling: "read", budgets: {}, instructions: undefined, enabled: true, aliases: {}, auth_providers: [] };
}

const slugInput = ref(props.slug ?? "");
const form = ref<PackEndpoint>(blank());
const submitting = ref(false);
const errors = ref<string[]>([]);
const loading = ref(props.mode === "edit");

onMounted(async () => {
  await Promise.all([apiCalls.fetchList(), scripts.fetchList(), authProviders.fetchList()]);
  if (props.mode === "edit" && props.slug) {
    const existing = await store.fetchOne(props.slug);
    if (existing) form.value = { ...existing };
    loading.value = false;
  } else {
    form.value = blank();
  }
});

const slugProblem = computed(() => (props.mode === "create" ? slugError(slugInput.value) : null));

function toggleAuthProvider(slug: string, on: boolean): void {
  form.value.auth_providers = on
    ? [...form.value.auth_providers, slug]
    : form.value.auth_providers.filter((s) => s !== slug);
}

async function submit(): Promise<void> {
  if (slugProblem.value) return;
  submitting.value = true;
  errors.value = [];
  try {
    if (props.mode === "create") {
      await store.create(slugInput.value, form.value);
      router.push(`/endpoints/${slugInput.value}`);
    } else if (props.slug) {
      await store.update(props.slug, form.value);
      router.push(`/endpoints/${props.slug}`);
    }
  } catch (e) {
    errors.value = e instanceof ApiError ? e.messages : ["request failed"];
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <LoadingState v-if="loading" label="loading endpoint" />
  <div v-else>
    <PageHeader
      :title="mode === 'create' ? 'New Endpoint' : `Edit ${props.slug}`"
      subtitle="endpoint"
      :back="{ to: mode === 'create' ? '/endpoints' : `/endpoints/${props.slug}`, label: mode === 'create' ? 'endpoints' : props.slug! }"
    />

    <ValidationErrors :errors="errors" />

    <form class="flex flex-col gap-6" @submit.prevent="submit">
      <DetailSection title="selection">
        <div class="flex flex-col gap-4">
          <FormField v-if="mode === 'create'" label="slug" required :errors="slugProblem ? [slugProblem] : []">
            <input v-model="slugInput" class="max-w-64 font-mono" :class="fieldClass" placeholder="my-endpoint" />
          </FormField>
          <p v-else class="font-mono text-sm text-ink">{{ props.slug }}</p>

          <FormField label="tag expression" required help="Boolean expression over api_call/script tags, e.g. has(demo) and not has(deprecated).">
            <input v-model="form.tag_expr" :class="fieldClass + ' font-mono'" placeholder="has(demo)" />
          </FormField>

          <div class="grid grid-cols-2 gap-4">
            <FormField label="write ceiling" required>
              <select v-model="form.write_ceiling" :class="fieldClass">
                <option value="read">read</option>
                <option value="write">write</option>
              </select>
            </FormField>
            <FormField label="enabled">
              <label class="flex items-center gap-2 text-sm text-ink">
                <input v-model="form.enabled" type="checkbox" :class="checkboxClass" />
                serves /mcp/{{ slugInput || props.slug }}
              </label>
            </FormField>
          </div>

          <FormField label="instructions">
            <textarea v-model="form.instructions" rows="3" :class="textareaClass" />
          </FormField>
        </div>
      </DetailSection>

      <DetailSection title="scoped auth providers">
        <p class="mb-3 text-xs text-ink-dim">Empty means every provider belonging to a selected service.</p>
        <div class="flex flex-col gap-1.5">
          <label v-for="p in authProviders.items" :key="p.slug" class="flex items-center gap-2 text-sm text-ink-dim">
            <input
              type="checkbox"
              :class="checkboxClass"
              :checked="form.auth_providers.includes(p.slug)"
              @change="toggleAuthProvider(p.slug, ($event.target as HTMLInputElement).checked)"
            />
            <span class="font-mono">{{ p.slug }}</span>
          </label>
          <p v-if="authProviders.items.length === 0" class="text-xs text-ink-faint italic">none defined</p>
        </div>
      </DetailSection>

      <DetailSection title="aliases">
        <p class="mb-3 text-xs text-ink-dim">Renames a tool's exposed name away from its own slug.</p>
        <AliasEditor v-model="form.aliases" :api-calls="apiCalls.items" :scripts="scripts.items" />
      </DetailSection>

      <DetailSection title="budgets">
        <BudgetsEditor v-model="form.budgets" />
      </DetailSection>

      <div class="flex items-center gap-3">
        <button type="submit" :disabled="submitting" :class="primaryButtonClass">
          {{ submitting ? "saving…" : "save" }}
        </button>
        <RouterLink :to="mode === 'create' ? '/endpoints' : `/endpoints/${props.slug}`" :class="ghostButtonClass">cancel</RouterLink>
      </div>
    </form>
  </div>
</template>
