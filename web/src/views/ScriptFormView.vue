<script setup lang="ts">
// New/edit form for a `PackScript`: source, declared callable api_calls (I1 — the full set of
// HTTP calls this script can ever make), params and budgets.
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { ApiError } from "@/api";
import type { PackScript } from "@/api";
import { useScriptsStore } from "@/stores/scripts";
import { useApiCallsStore } from "@/stores/apiCalls";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import LoadingState from "@/components/LoadingState.vue";
import FormField from "@/components/form/FormField.vue";
import ValidationErrors from "@/components/form/ValidationErrors.vue";
import StringListEditor from "@/components/form/StringListEditor.vue";
import ParamEditor from "@/components/form/ParamEditor.vue";
import CallableEditor from "@/components/form/CallableEditor.vue";
import BudgetsEditor from "@/components/form/BudgetsEditor.vue";
import InputSchemaPreview from "@/components/form/InputSchemaPreview.vue";
import { fieldClass, ghostButtonClass, primaryButtonClass, textareaClass } from "@/lib/formStyle";
import { slugError } from "@/lib/slug";

const props = defineProps<{ mode: "create" | "edit"; slug?: string }>();
const router = useRouter();
const store = useScriptsStore();
const apiCalls = useApiCallsStore();

function blank(): PackScript {
  return { source: "", params: [], callable: {}, budgets: {}, description: undefined, tags: [] };
}

const slugInput = ref(props.slug ?? "");
const form = ref<PackScript>(blank());
const submitting = ref(false);
const errors = ref<string[]>([]);
const loading = ref(props.mode === "edit");

onMounted(async () => {
  await apiCalls.fetchList();
  if (props.mode === "edit" && props.slug) {
    const existing = await store.fetchOne(props.slug);
    if (existing) form.value = { ...existing };
    loading.value = false;
  } else {
    form.value = blank();
  }
});

const slugProblem = computed(() => (props.mode === "create" ? slugError(slugInput.value) : null));

async function submit(): Promise<void> {
  if (slugProblem.value) return;
  submitting.value = true;
  errors.value = [];
  try {
    if (props.mode === "create") {
      await store.create(slugInput.value, form.value);
      router.push(`/scripts/${slugInput.value}`);
    } else if (props.slug) {
      await store.update(props.slug, form.value);
      router.push(`/scripts/${props.slug}`);
    }
  } catch (e) {
    errors.value = e instanceof ApiError ? e.messages : ["request failed"];
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <LoadingState v-if="loading" label="loading script" />
  <div v-else>
    <PageHeader
      :title="mode === 'create' ? 'New Script' : `Edit ${props.slug}`"
      subtitle="script — Rhai compositions of declared api_calls, never raw HTTP (I1)"
      :back="{ to: mode === 'create' ? '/scripts' : `/scripts/${props.slug}`, label: mode === 'create' ? 'scripts' : props.slug! }"
    />

    <ValidationErrors :errors="errors" />

    <form class="flex flex-col gap-6" @submit.prevent="submit">
      <DetailSection title="identity">
        <div class="flex flex-col gap-4">
          <FormField v-if="mode === 'create'" label="slug" required :errors="slugProblem ? [slugProblem] : []">
            <input v-model="slugInput" class="max-w-64 font-mono" :class="fieldClass" placeholder="my-script" />
          </FormField>
          <p v-else class="font-mono text-sm text-ink">{{ props.slug }}</p>
          <FormField label="description">
            <textarea v-model="form.description" rows="2" :class="textareaClass" />
          </FormField>
          <FormField label="tags">
            <StringListEditor v-model="form.tags" placeholder="tag" />
          </FormField>
        </div>
      </DetailSection>

      <DetailSection title="callable api_calls (I1)">
        <p class="mb-3 text-xs text-ink-dim">
          The full set of HTTP calls this script can ever make — `api()`/`api_many()` in the
          source below can only address these aliases, never an arbitrary URL.
        </p>
        <CallableEditor v-model="form.callable" :api-calls="apiCalls.items" />
      </DetailSection>

      <DetailSection title="source">
        <textarea
          v-model="form.source"
          rows="16"
          spellcheck="false"
          :class="textareaClass + ' font-mono'"
          placeholder="let listing = api(&quot;list&quot;, #{});&#10;#{ &quot;count&quot;: listing.len() }"
        />
      </DetailSection>

      <DetailSection title="parameters">
        <p class="mb-3 text-xs text-ink-dim">Every script param binds locally — never into an HTTP request directly.</p>
        <ParamEditor v-model="form.params" :allowed-locations="['local']" />
      </DetailSection>

      <DetailSection title="generated input schema">
        <InputSchemaPreview :params="form.params" />
      </DetailSection>

      <DetailSection title="own budget">
        <BudgetsEditor v-model="form.budgets" />
      </DetailSection>

      <div class="flex items-center gap-3">
        <button type="submit" :disabled="submitting" :class="primaryButtonClass">
          {{ submitting ? "saving…" : "save" }}
        </button>
        <RouterLink :to="mode === 'create' ? '/scripts' : `/scripts/${props.slug}`" :class="ghostButtonClass">cancel</RouterLink>
      </div>
    </form>
  </div>
</template>
