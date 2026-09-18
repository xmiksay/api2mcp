<script setup lang="ts">
// New/edit form for a `PackApiCall` — the screen where curation actually happens (per the
// chunk brief): the param editor below makes definer-fixed vs caller-supplied unmistakable while
// editing, and the generated `inputSchema` is recomputed live as params change.
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { ApiError } from "@/api";
import type { PackApiCall, PackProjectionField } from "@/api";
import { useApiCallsStore } from "@/stores/apiCalls";
import { useServicesStore } from "@/stores/services";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import LoadingState from "@/components/LoadingState.vue";
import FormField from "@/components/form/FormField.vue";
import ValidationErrors from "@/components/form/ValidationErrors.vue";
import KeyValueEditor from "@/components/form/KeyValueEditor.vue";
import StringListEditor from "@/components/form/StringListEditor.vue";
import ParamEditor from "@/components/form/ParamEditor.vue";
import ProjectionEditor from "@/components/form/ProjectionEditor.vue";
import PaginationEditor from "@/components/form/PaginationEditor.vue";
import InputSchemaPreview from "@/components/form/InputSchemaPreview.vue";
import { checkboxClass, fieldClass, ghostButtonClass, primaryButtonClass, textareaClass } from "@/lib/formStyle";
import { slugError } from "@/lib/slug";

const props = defineProps<{ mode: "create" | "edit"; slug?: string }>();
const router = useRouter();
const store = useApiCallsStore();
const services = useServicesStore();

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

function blank(): PackApiCall {
  return {
    service: services.items[0]?.slug ?? "",
    method: "GET",
    path_template: "/",
    query_fixed: {},
    body_template: undefined,
    access: "read",
    idempotent: true,
    projection: undefined,
    pagination: { kind: "none" },
    timeout_ms: undefined,
    max_response_bytes: undefined,
    params: [],
    tags: [],
    description: undefined,
  };
}

const slugInput = ref(props.slug ?? "");
const form = ref<PackApiCall>(blank());
const bodyTemplateText = ref("");
const bodyTemplateError = ref<string | null>(null);
const submitting = ref(false);
const errors = ref<string[]>([]);
const loading = ref(props.mode === "edit");

onMounted(async () => {
  await services.fetchList();
  if (props.mode === "edit" && props.slug) {
    const existing = await store.fetchOne(props.slug);
    if (existing) {
      form.value = { ...existing };
      if (existing.body_template !== undefined) {
        bodyTemplateText.value = JSON.stringify(existing.body_template, null, 2);
      }
    }
    loading.value = false;
  } else {
    form.value = blank();
  }
});

const slugProblem = computed(() => (props.mode === "create" ? slugError(slugInput.value) : null));
const hasProjection = computed({
  get: () => form.value.projection !== undefined,
  set: (on) => (form.value.projection = on ? { fields: [] } : undefined),
});
const projectionFields = computed<PackProjectionField[]>({
  get: () => form.value.projection?.fields ?? [],
  set: (fields) => {
    if (form.value.projection) form.value.projection = { fields };
  },
});
const timeoutOverrideEnabled = computed({
  get: () => form.value.timeout_ms !== undefined,
  set: (on) => (form.value.timeout_ms = on ? 5000 : undefined),
});
const maxBytesOverrideEnabled = computed({
  get: () => form.value.max_response_bytes !== undefined,
  set: (on) => (form.value.max_response_bytes = on ? 1_048_576 : undefined),
});

/** The whole-segment placeholders in `path_template` — mirrors `validate::items::validate_api_call`'s
 * check that these exactly equal the `location: path` params, surfaced here before the round trip. */
const pathPlaceholders = computed(() => {
  const matches = form.value.path_template.match(/\{[^}/]+\}/g) ?? [];
  return new Set(matches.map((m) => m.slice(1, -1)));
});
const pathParamNames = computed(
  () => new Set(form.value.params.filter((p) => p.location === "path").map((p) => p.name)),
);
const pathMismatch = computed(() => {
  const a = [...pathPlaceholders.value].sort();
  const b = [...pathParamNames.value].sort();
  return JSON.stringify(a) !== JSON.stringify(b);
});

function syncBodyTemplate(): boolean {
  const text = bodyTemplateText.value.trim();
  if (text.length === 0) {
    form.value.body_template = undefined;
    bodyTemplateError.value = null;
    return true;
  }
  try {
    form.value.body_template = JSON.parse(text);
    bodyTemplateError.value = null;
    return true;
  } catch (e) {
    bodyTemplateError.value = e instanceof Error ? e.message : "invalid JSON";
    return false;
  }
}

async function submit(): Promise<void> {
  if (slugProblem.value || !syncBodyTemplate()) return;
  submitting.value = true;
  errors.value = [];
  try {
    if (props.mode === "create") {
      await store.create(slugInput.value, form.value);
      router.push(`/api-calls/${slugInput.value}`);
    } else if (props.slug) {
      await store.update(props.slug, form.value);
      router.push(`/api-calls/${props.slug}`);
    }
  } catch (e) {
    errors.value = e instanceof ApiError ? e.messages : ["request failed"];
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <LoadingState v-if="loading" label="loading api call" />
  <div v-else>
    <PageHeader
      :title="mode === 'create' ? 'New Api Call' : `Edit ${props.slug}`"
      subtitle="api call"
      :back="{ to: mode === 'create' ? '/api-calls' : `/api-calls/${props.slug}`, label: mode === 'create' ? 'api calls' : props.slug! }"
    />

    <ValidationErrors :errors="errors" />

    <form class="flex flex-col gap-6" @submit.prevent="submit">
      <DetailSection title="request">
        <div class="flex flex-col gap-4">
          <FormField v-if="mode === 'create'" label="slug" required :errors="slugProblem ? [slugProblem] : []">
            <input v-model="slugInput" class="max-w-64 font-mono" :class="fieldClass" placeholder="my-api-call" />
          </FormField>
          <p v-else class="font-mono text-sm text-ink">{{ props.slug }}</p>

          <div class="grid grid-cols-2 gap-4">
            <FormField label="service" required>
              <select v-model="form.service" :class="fieldClass">
                <option v-for="s in services.items" :key="s.slug" :value="s.slug">{{ s.slug }}</option>
              </select>
            </FormField>
            <FormField label="method" required>
              <select v-model="form.method" :class="fieldClass">
                <option v-for="m in METHODS" :key="m" :value="m">{{ m }}</option>
              </select>
            </FormField>
            <FormField label="access" required>
              <select v-model="form.access" :class="fieldClass">
                <option value="read">read</option>
                <option value="write">write</option>
              </select>
            </FormField>
          </div>

          <FormField
            label="path template"
            required
            help="Whole-segment placeholders only, e.g. /items/{id}."
            :errors="pathMismatch ? [`path placeholders {${[...pathPlaceholders].join(', ')}} must exactly match this call's location:path params {${[...pathParamNames].join(', ')}}`] : []"
          >
            <input v-model="form.path_template" :class="fieldClass + ' font-mono'" placeholder="/items/{id}" />
          </FormField>

          <FormField label="fixed query parameters">
            <KeyValueEditor v-model="form.query_fixed" />
          </FormField>

          <FormField label="body template (JSON, optional)" :errors="bodyTemplateError ? [bodyTemplateError] : []">
            <textarea v-model="bodyTemplateText" rows="4" :class="textareaClass + ' font-mono'" placeholder="{}" @blur="syncBodyTemplate" />
          </FormField>

          <label class="flex items-center gap-2 text-xs text-ink-dim">
            <input v-model="form.idempotent" type="checkbox" :class="checkboxClass" />
            idempotent
          </label>

          <FormField label="description" help="What a model reads to decide whether and when to call this tool.">
            <textarea v-model="form.description" rows="2" :class="textareaClass" />
          </FormField>

          <FormField label="tags">
            <StringListEditor v-model="form.tags" placeholder="tag" />
          </FormField>
        </div>
      </DetailSection>

      <DetailSection title="parameters">
        <ParamEditor v-model="form.params" :allowed-locations="['path', 'query', 'header', 'body']" />
      </DetailSection>

      <DetailSection title="generated input schema">
        <p class="mb-3 text-xs text-ink-dim">What a model would be handed right now, recomputed as params change above.</p>
        <InputSchemaPreview :params="form.params" />
      </DetailSection>

      <DetailSection title="projection">
        <label class="mb-3 flex items-center gap-2 text-xs text-ink-dim">
          <input v-model="hasProjection" type="checkbox" :class="checkboxClass" />
          apply a projection (otherwise the raw upstream response passes through unmodified)
        </label>
        <ProjectionEditor v-if="hasProjection" v-model="projectionFields" />
      </DetailSection>

      <DetailSection title="pagination">
        <PaginationEditor v-model="form.pagination" />
      </DetailSection>

      <DetailSection title="limits (optional overrides of the service's own)">
        <div class="grid grid-cols-2 gap-4">
          <FormField label="timeout (ms)">
            <label class="mb-1 flex items-center gap-2 text-xs text-ink-dim">
              <input v-model="timeoutOverrideEnabled" type="checkbox" :class="checkboxClass" />
              override
            </label>
            <input v-if="timeoutOverrideEnabled" v-model.number="form.timeout_ms" type="number" min="1" :class="fieldClass" />
          </FormField>
          <FormField label="max response bytes">
            <label class="mb-1 flex items-center gap-2 text-xs text-ink-dim">
              <input v-model="maxBytesOverrideEnabled" type="checkbox" :class="checkboxClass" />
              override
            </label>
            <input v-if="maxBytesOverrideEnabled" v-model.number="form.max_response_bytes" type="number" min="1" :class="fieldClass" />
          </FormField>
        </div>
      </DetailSection>

      <div class="flex items-center gap-3">
        <button type="submit" :disabled="submitting" :class="primaryButtonClass">
          {{ submitting ? "saving…" : "save" }}
        </button>
        <RouterLink :to="mode === 'create' ? '/api-calls' : `/api-calls/${props.slug}`" :class="ghostButtonClass">cancel</RouterLink>
      </div>
    </form>
  </div>
</template>
