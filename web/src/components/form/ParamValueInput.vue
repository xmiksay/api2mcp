<script setup lang="ts">
// A single typed JSON value editor for a `Param`'s `default`/`fixed`/enum-member slots — the
// control shown depends on the param's own declared `type`, so a `boolean` param edits its fixed
// value as a checkbox, an `integer` as a number input, and so on, rather than one raw JSON
// textarea for every type.
import type { ParamType } from "@/api";
import { checkboxClass, fieldClass } from "@/lib/formStyle";

defineProps<{ type: ParamType; modelValue: unknown }>();
const emit = defineEmits<{ "update:modelValue": [unknown] }>();

function onText(raw: string): void {
  emit("update:modelValue", raw);
}
function onNumber(raw: string): void {
  const n = Number(raw);
  emit("update:modelValue", raw === "" || !Number.isFinite(n) ? 0 : n);
}
function onBool(checked: boolean): void {
  emit("update:modelValue", checked);
}
function onArray(raw: string): void {
  emit(
    "update:modelValue",
    raw
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0),
  );
}
function arrayText(v: unknown): string {
  return Array.isArray(v) ? v.join(", ") : "";
}
</script>

<template>
  <input
    v-if="type === 'string'"
    :value="typeof modelValue === 'string' ? modelValue : ''"
    :class="fieldClass"
    @input="onText(($event.target as HTMLInputElement).value)"
  />
  <input
    v-else-if="type === 'integer'"
    type="number"
    step="1"
    :value="typeof modelValue === 'number' ? modelValue : 0"
    :class="fieldClass"
    @input="onNumber(($event.target as HTMLInputElement).value)"
  />
  <input
    v-else-if="type === 'number'"
    type="number"
    step="any"
    :value="typeof modelValue === 'number' ? modelValue : 0"
    :class="fieldClass"
    @input="onNumber(($event.target as HTMLInputElement).value)"
  />
  <label v-else-if="type === 'boolean'" class="flex items-center gap-2 text-sm text-ink">
    <input
      type="checkbox"
      :class="checkboxClass"
      :checked="modelValue === true"
      @change="onBool(($event.target as HTMLInputElement).checked)"
    />
    {{ modelValue === true ? "true" : "false" }}
  </label>
  <input
    v-else
    :value="arrayText(modelValue)"
    placeholder="comma-separated"
    :class="fieldClass"
    @input="onArray(($event.target as HTMLInputElement).value)"
  />
</template>
