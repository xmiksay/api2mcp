<script setup lang="ts">
// Builds a test-run `args` object straight from a definition's own caller-visible params — the
// same params `lib/inputSchema.ts` turns into the generated `inputSchema`, so this form asks for
// exactly what a real MCP caller would be asked for, no more. A fixed param never appears here:
// it isn't something a caller (or a tester standing in for one) can supply.
import { computed, reactive, watch } from "vue";
import type { PackParam, ParamType } from "@/api";
import { checkboxClass } from "@/lib/formStyle";
import ParamValueInput from "./ParamValueInput.vue";

const props = defineProps<{ params: PackParam[] }>();

const visible = computed(() => [...props.params].filter((p) => p.fixed === undefined).sort((a, b) => a.position - b.position));

function defaultFor(ty: ParamType): unknown {
  switch (ty) {
    case "boolean":
      return false;
    case "integer":
    case "number":
      return 0;
    case "string_array":
      return [];
    default:
      return "";
  }
}

const state = reactive<Record<string, { value: unknown; included: boolean }>>({});

watch(
  visible,
  (list) => {
    for (const p of list) {
      if (!(p.name in state)) {
        state[p.name] = {
          value: p.default ?? defaultFor(p.type),
          included: p.required || p.default !== undefined,
        };
      }
    }
  },
  { immediate: true },
);

const args = computed<Record<string, unknown>>(() => {
  const out: Record<string, unknown> = {};
  for (const p of visible.value) {
    const s = state[p.name];
    if (s?.included) out[p.name] = s.value;
  }
  return out;
});

const missing = computed(() => visible.value.filter((p) => p.required && !state[p.name]?.included).map((p) => p.name));
const isValid = computed(() => missing.value.length === 0);

defineExpose({ args, isValid, missing });
</script>

<template>
  <div v-if="visible.length === 0" class="text-xs text-ink-faint italic">
    this tool takes no caller-supplied arguments
  </div>
  <div v-else class="flex flex-col gap-2">
    <div v-for="p in visible" :key="p.name" class="flex items-center gap-3">
      <span class="w-32 shrink-0 truncate font-mono text-xs text-ink-dim" :title="p.name">
        {{ p.name }}<span v-if="p.required" class="text-write">*</span>
      </span>
      <input
        v-if="!p.required"
        type="checkbox"
        :class="checkboxClass"
        :checked="state[p.name]?.included ?? false"
        title="include this argument"
        @change="state[p.name].included = ($event.target as HTMLInputElement).checked"
      />
      <ParamValueInput
        v-if="state[p.name]?.included"
        :type="p.type"
        :model-value="state[p.name].value"
        @update:model-value="(v) => (state[p.name].value = v)"
      />
      <span v-else class="text-xs text-ink-faint italic">omitted — server default applies</span>
    </div>
    <p v-if="missing.length > 0" class="text-xs text-error">missing required: {{ missing.join(", ") }}</p>
  </div>
</template>
