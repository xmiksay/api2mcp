<script setup lang="ts">
// `POST /api/scripts/{slug}/test` — same "real run, real upstream" contract as the api_call test
// panel, but a script's own outcome is a returned value or a structured `ScriptFailure`, not a
// raw/projected pair, so this renders that shape instead of forcing ApiCallTestPanel's layout.
import { computed, ref } from "vue";
import { ApiError, scriptsApi } from "@/api";
import type { ScriptTestResult, ScriptView } from "@/api";
import type { ToolExposure } from "@/composables/useToolExposure";
import { runStatusTone } from "@/lib/tone";
import { fieldClass, primaryButtonClass } from "@/lib/formStyle";
import ArgsForm from "@/components/form/ArgsForm.vue";
import JsonViewer from "@/components/JsonViewer.vue";
import StatusPill from "@/components/StatusPill.vue";
import ValidationErrors from "@/components/form/ValidationErrors.vue";
import ScriptFailureView from "@/components/ScriptFailureView.vue";
import ScriptCallBreakdown from "@/components/ScriptCallBreakdown.vue";

const props = defineProps<{ script: ScriptView; exposures: ToolExposure[] }>();

const endpointSlug = ref(props.exposures[0]?.endpointSlug ?? "");
const argsForm = ref<InstanceType<typeof ArgsForm> | null>(null);
const running = ref(false);
const requestErrors = ref<string[] | null>(null);
const result = ref<ScriptTestResult | null>(null);

const canRun = computed(() => endpointSlug.value.length > 0 && !running.value);

async function run(): Promise<void> {
  if (!argsForm.value || !endpointSlug.value) return;
  running.value = true;
  requestErrors.value = null;
  try {
    result.value = await scriptsApi.test(props.script.slug, endpointSlug.value, argsForm.value.args);
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
      no enabled endpoint currently exposes this script as a tool — add it to an endpoint's tag
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

      <ArgsForm ref="argsForm" :params="script.params" />

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
        </div>

        <ScriptFailureView v-if="result.failure" :source="script.source" :failure="result.failure" />
        <div v-else>
          <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">returned value</div>
          <JsonViewer :value="result.value" />
        </div>

        <div>
          <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">upstream calls (input order)</div>
          <ScriptCallBreakdown :calls="result.calls" />
        </div>
      </div>
    </template>
  </div>
</template>
