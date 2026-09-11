<script setup lang="ts">
// Editable `PackBudgets` — the same five optional axes `BudgetsSummary` renders read-only.
// Each axis is "unset" (the ceiling above it applies instead) or a concrete number; a checkbox
// is the honest UI for that, rather than overloading an empty text input with the same meaning.
import { computed } from "vue";
import type { PackBudgets } from "@/api";
import { checkboxClass, fieldClass } from "@/lib/formStyle";

const props = defineProps<{ modelValue: PackBudgets }>();
const emit = defineEmits<{ "update:modelValue": [PackBudgets] }>();

type Key = keyof PackBudgets;
const axes: { key: Key; label: string; help: string }[] = [
  { key: "max_calls", label: "max calls", help: "total upstream calls this run may make" },
  { key: "max_bytes", label: "max bytes", help: "total response bytes across every call" },
  { key: "wall_clock_ms", label: "wall clock (ms)", help: "total run duration" },
  { key: "max_pages", label: "max pages", help: "pagination pages followed" },
  { key: "max_concurrency", label: "max concurrency", help: "in-flight upstream calls at once" },
];

function enabled(key: Key): boolean {
  return props.modelValue[key] !== undefined;
}

function toggle(key: Key, on: boolean): void {
  emit("update:modelValue", { ...props.modelValue, [key]: on ? 1 : undefined });
}

function setValue(key: Key, raw: string): void {
  const n = Number(raw);
  emit("update:modelValue", { ...props.modelValue, [key]: Number.isFinite(n) && n >= 0 ? n : undefined });
}

const rows = computed(() => axes);
</script>

<template>
  <div class="flex flex-col gap-2">
    <div v-for="axis in rows" :key="axis.key" class="flex items-center gap-3">
      <input
        type="checkbox"
        :class="checkboxClass"
        :checked="enabled(axis.key)"
        @change="toggle(axis.key, ($event.target as HTMLInputElement).checked)"
      />
      <span class="w-32 shrink-0 text-xs text-ink-dim">{{ axis.label }}</span>
      <input
        type="number"
        min="0"
        :disabled="!enabled(axis.key)"
        :value="modelValue[axis.key] ?? ''"
        :class="fieldClass + ' max-w-40'"
        @input="setValue(axis.key, ($event.target as HTMLInputElement).value)"
      />
      <span class="text-xs text-ink-faint">{{ axis.help }}</span>
    </div>
  </div>
</template>
