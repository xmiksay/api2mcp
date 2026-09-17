<script setup lang="ts">
// Generates a token. Deliberately does not hold the plaintext response itself — it hands the
// full `TokenCreateResponse` up to the parent view via `created` and resets, so the one-time
// reveal lives in exactly one place (TokenRevealPanel), not duplicated here.
import { computed, onMounted, ref } from "vue";
import { ApiError } from "@/api";
import type { TokenCreateResponse } from "@/api";
import { useTokensStore } from "@/stores/tokens";
import { useEndpointsStore } from "@/stores/endpoints";
import DetailSection from "@/components/DetailSection.vue";
import FormField from "@/components/form/FormField.vue";
import ValidationErrors from "@/components/form/ValidationErrors.vue";
import { checkboxClass, fieldClass, ghostButtonClass, writeButtonClass } from "@/lib/formStyle";

const emit = defineEmits<{ created: [TokenCreateResponse]; cancel: [] }>();

const store = useTokensStore();
const endpoints = useEndpointsStore();
onMounted(() => endpoints.fetchList());

const label = ref("");
const expiryChoice = ref<"30" | "60" | "90" | "never">("30");
const scope = ref<"all" | "specific">("all");
// Off by default, matching the server: a token that can rewrite definitions is a different thing
// from one that can call tools, and it should be a deliberate choice rather than a default.
const controlPlane = ref(false);
const selected = ref<string[]>([]);

const submitting = ref(false);
const errors = ref<string[]>([]);

const scopeProblem = computed(() =>
  scope.value === "specific" && selected.value.length === 0 ? "select at least one endpoint" : null,
);
const labelProblem = computed(() => (label.value.trim() === "" ? "label is required" : null));

function toggle(slug: string, on: boolean): void {
  selected.value = on ? [...selected.value, slug] : selected.value.filter((s) => s !== slug);
}

function reset(): void {
  label.value = "";
  expiryChoice.value = "30";
  scope.value = "all";
  selected.value = [];
  controlPlane.value = false;
}

async function submit(): Promise<void> {
  if (scopeProblem.value || labelProblem.value) return;
  submitting.value = true;
  errors.value = [];
  try {
    const created = await store.create({
      label: label.value.trim(),
      expires_in_days: expiryChoice.value === "never" ? null : Number(expiryChoice.value),
      endpoints: scope.value === "all" ? [] : selected.value,
      control_plane: controlPlane.value,
    });
    reset();
    emit("created", created);
  } catch (e) {
    errors.value = e instanceof ApiError ? e.messages : ["request failed"];
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <DetailSection title="generate token">
    <ValidationErrors :errors="errors" />
    <form class="flex flex-col gap-4" @submit.prevent="submit">
      <FormField label="label" required help="How you'll recognize this token later — e.g. the client or machine it's for.">
        <input v-model="label" :class="fieldClass" placeholder="laptop-claude-desktop" />
      </FormField>

      <FormField label="expires" required>
        <select v-model="expiryChoice" :class="fieldClass + ' max-w-48'">
          <option value="30">30 days</option>
          <option value="60">60 days</option>
          <option value="90">90 days</option>
          <option value="never">never</option>
        </select>
      </FormField>

      <FormField label="endpoints" required :errors="scopeProblem ? [scopeProblem] : []">
        <div class="flex flex-col gap-2">
          <label class="flex items-center gap-2 text-sm text-ink">
            <input v-model="scope" type="radio" value="all" class="accent-read" />
            all endpoints
            <span class="text-xs text-ink-faint">— including any created after this token, today</span>
          </label>
          <label class="flex items-center gap-2 text-sm text-ink">
            <input v-model="scope" type="radio" value="specific" class="accent-read" />
            specific endpoints
          </label>
          <div v-if="scope === 'specific'" class="ml-6 flex flex-col gap-1.5 border-l border-border pl-3">
            <label v-for="e in endpoints.items" :key="e.slug" class="flex items-center gap-2 text-sm text-ink-dim">
              <input
                type="checkbox"
                :class="checkboxClass"
                :checked="selected.includes(e.slug)"
                @change="toggle(e.slug, ($event.target as HTMLInputElement).checked)"
              />
              <span class="font-mono">{{ e.slug }}</span>
            </label>
            <p v-if="endpoints.items.length === 0" class="text-xs text-ink-faint italic">no endpoints defined</p>
          </div>
        </div>
      </FormField>

      <FormField label="control plane">
        <label class="flex items-start gap-2 text-sm text-ink">
          <input v-model="controlPlane" type="checkbox" :class="checkboxClass" class="mt-0.5" />
          <span>
            may define services, api_calls, scripts and endpoints
            <span class="block text-xs text-ink-faint">
              needed to connect an MCP client to <span class="font-mono">/mcp</span>, the factory.
              Without it this token can only call tools on the endpoints granted above, and
              <span class="font-mono">/mcp</span> reports itself as not existing.
            </span>
          </span>
        </label>
      </FormField>

      <div class="flex items-center gap-3">
        <button type="submit" :disabled="submitting" :class="writeButtonClass">
          {{ submitting ? "generating…" : "generate" }}
        </button>
        <button type="button" :class="ghostButtonClass" @click="emit('cancel')">cancel</button>
      </div>
    </form>
  </DetailSection>
</template>
