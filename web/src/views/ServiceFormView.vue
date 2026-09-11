<script setup lang="ts">
// New/edit form for a `PackService` — base URL, allowlisted origins, transport limits.
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { ApiError } from "@/api";
import type { PackService } from "@/api";
import { useServicesStore } from "@/stores/services";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import FormField from "@/components/form/FormField.vue";
import ValidationErrors from "@/components/form/ValidationErrors.vue";
import StringListEditor from "@/components/form/StringListEditor.vue";
import KeyValueEditor from "@/components/form/KeyValueEditor.vue";
import { checkboxClass, fieldClass, ghostButtonClass, primaryButtonClass } from "@/lib/formStyle";
import { slugError } from "@/lib/slug";

const props = defineProps<{ mode: "create" | "edit"; slug?: string }>();
const router = useRouter();
const store = useServicesStore();

function blank(): PackService {
  return {
    base_url: "",
    origin_allowlist: [],
    default_headers: {},
    timeout_ms: 5000,
    max_concurrency: 4,
    rate_limit_per_min: undefined,
    max_response_bytes: 1_048_576,
  };
}

const slugInput = ref(props.slug ?? "");
const form = ref<PackService>(blank());
const submitting = ref(false);
const errors = ref<string[]>([]);
const loading = ref(props.mode === "edit");

onMounted(async () => {
  if (props.mode === "edit" && props.slug) {
    const existing = await store.fetchOne(props.slug);
    if (existing) form.value = { ...existing };
    loading.value = false;
  }
});

const slugProblem = computed(() => (props.mode === "create" ? slugError(slugInput.value) : null));
const rateLimitEnabled = computed({
  get: () => form.value.rate_limit_per_min !== undefined,
  set: (on) => (form.value.rate_limit_per_min = on ? 60 : undefined),
});

function addOwnOrigin(): void {
  try {
    const origin = new URL(form.value.base_url).origin;
    if (!form.value.origin_allowlist.includes(origin)) {
      form.value.origin_allowlist = [...form.value.origin_allowlist, origin];
    }
  } catch {
    // Not a parseable URL yet — nothing to derive an origin from.
  }
}

async function submit(): Promise<void> {
  if (slugProblem.value) return;
  submitting.value = true;
  errors.value = [];
  try {
    if (props.mode === "create") {
      await store.create(slugInput.value, form.value);
      router.push(`/services/${slugInput.value}`);
    } else if (props.slug) {
      await store.update(props.slug, form.value);
      router.push(`/services/${props.slug}`);
    }
  } catch (e) {
    errors.value = e instanceof ApiError ? e.messages : ["request failed"];
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <LoadingState v-if="loading" label="loading service" />
  <ErrorState v-else-if="store.detailError" :message="store.detailError" @retry="() => router.go(0)" />
  <div v-else>
    <PageHeader
      :title="mode === 'create' ? 'New Service' : `Edit ${props.slug}`"
      subtitle="service"
      :back="{ to: mode === 'create' ? '/services' : `/services/${props.slug}`, label: mode === 'create' ? 'services' : props.slug! }"
    />

    <ValidationErrors :errors="errors" />

    <form class="flex flex-col gap-6" @submit.prevent="submit">
      <DetailSection title="identity">
        <FormField v-if="mode === 'create'" label="slug" required :errors="slugProblem ? [slugProblem] : []">
          <input v-model="slugInput" class="max-w-64 font-mono" :class="fieldClass" placeholder="my-service" />
        </FormField>
        <p v-else class="font-mono text-sm text-ink">{{ props.slug }}</p>
      </DetailSection>

      <DetailSection title="transport">
        <div class="flex flex-col gap-4">
          <FormField label="base url" required>
            <input v-model="form.base_url" :class="fieldClass + ' font-mono'" placeholder="https://api.example.com" />
          </FormField>
          <FormField label="origin allowlist" required help="Every origin a request built from this service may resolve to (I2). Must include the base url's own origin.">
            <StringListEditor v-model="form.origin_allowlist" mono placeholder="https://api.example.com" />
            <button type="button" :class="ghostButtonClass + ' mt-1 self-start'" @click="addOwnOrigin">+ add base url's own origin</button>
          </FormField>
          <FormField label="default headers">
            <KeyValueEditor v-model="form.default_headers" />
          </FormField>
          <div class="grid grid-cols-2 gap-4">
            <FormField label="timeout (ms)" required>
              <input type="number" min="1" v-model.number="form.timeout_ms" :class="fieldClass" />
            </FormField>
            <FormField label="max concurrency" required>
              <input type="number" min="1" v-model.number="form.max_concurrency" :class="fieldClass" />
            </FormField>
            <FormField label="max response bytes" required>
              <input type="number" min="1" v-model.number="form.max_response_bytes" :class="fieldClass" />
            </FormField>
            <FormField label="rate limit (per min)">
              <label class="mb-1 flex items-center gap-2 text-xs text-ink-dim">
                <input type="checkbox" v-model="rateLimitEnabled" :class="checkboxClass" />
                enabled
              </label>
              <input v-if="rateLimitEnabled" type="number" min="1" v-model.number="form.rate_limit_per_min" :class="fieldClass" />
            </FormField>
          </div>
        </div>
      </DetailSection>

      <div class="flex items-center gap-3">
        <button type="submit" :disabled="submitting" :class="primaryButtonClass">
          {{ submitting ? "saving…" : "save" }}
        </button>
        <RouterLink :to="mode === 'create' ? '/services' : `/services/${props.slug}`" :class="ghostButtonClass">cancel</RouterLink>
      </div>
    </form>
  </div>
</template>
