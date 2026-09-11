<script setup lang="ts">
// `POST /api/api_calls/{slug}/test` — run for real, against the real upstream. A failing test is
// a normal outcome (an upstream 404 is often exactly what someone is testing for), so this never
// renders a run's own `error`/non-ok status as a crash — only a request that couldn't even be
// attempted (no endpoint exposes this call, a network failure) gets the error-banner treatment.
import { computed, ref } from "vue";
import { apiCallsApi, ApiError } from "@/api";
import type { ApiCallTestResult, ApiCallView } from "@/api";
import type { ToolExposure } from "@/composables/useToolExposure";
import { runStatusTone } from "@/lib/tone";
import { fieldClass, primaryButtonClass } from "@/lib/formStyle";
import ArgsForm from "@/components/form/ArgsForm.vue";
import JsonViewer from "@/components/JsonViewer.vue";
import StatusPill from "@/components/StatusPill.vue";
import ValidationErrors from "@/components/form/ValidationErrors.vue";

const props = defineProps<{ apiCall: ApiCallView; exposures: ToolExposure[] }>();

const endpointSlug = ref(props.exposures[0]?.endpointSlug ?? "");
const argsForm = ref<InstanceType<typeof ArgsForm> | null>(null);
const running = ref(false);
const requestErrors = ref<string[] | null>(null);
const result = ref<ApiCallTestResult | null>(null);

const canRun = computed(() => endpointSlug.value.length > 0 && !running.value);

async function run(): Promise<void> {
  if (!argsForm.value || !endpointSlug.value) return;
  running.value = true;
  requestErrors.value = null;
  try {
    result.value = await apiCallsApi.test(props.apiCall.slug, endpointSlug.value, argsForm.value.args);
  } catch (e) {
    requestErrors.value = e instanceof ApiError ? e.messages : ["request failed"];
  } finally {
    running.value = false;
  }
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <p v-if="exposures.length === 0" class="text-xs text-ink-faint italic">
      no enabled endpoint currently exposes this api_call as a tool — add it to an endpoint's tag
      selection first, then come back here to test it.
    </p>
    <template v-else>
      <div class="flex items-center gap-3">
        <label class="text-[11px] tracking-[0.1em] text-ink-faint uppercase">run via endpoint</label>
        <select v-model="endpointSlug" :class="fieldClass + ' max-w-56'">
          <option v-for="exp in exposures" :key="exp.endpointSlug" :value="exp.endpointSlug">
            {{ exp.endpointSlug }} (as "{{ exp.tool.name }}")
          </option>
        </select>
      </div>

      <ArgsForm ref="argsForm" :params="apiCall.params" />

      <ValidationErrors v-if="requestErrors" :errors="requestErrors" />

      <button type="button" :disabled="!canRun" :class="primaryButtonClass + ' self-start'" @click="run">
        {{ running ? "running…" : "run test" }}
      </button>

      <div v-if="result" class="flex flex-col gap-3 border-t border-border pt-4">
        <div class="flex flex-wrap items-center gap-3">
          <StatusPill :label="result.status" :tone="runStatusTone(result.status)" />
          <RouterLink :to="`/runs/${result.run_id}`" class="text-xs text-read hover:underline">
            view run {{ result.run_id }}
          </RouterLink>
          <span v-if="result.error" class="text-xs text-ink-dim">{{ result.error }}</span>
        </div>
        <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
          <div>
            <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">raw upstream response</div>
            <JsonViewer :value="result.raw" null-label="no response body recorded" />
          </div>
          <div>
            <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">projected result</div>
            <JsonViewer :value="result.projected" null-label="no projected value" />
          </div>
        </div>
      </div>
    </template>
  </div>
</template>
