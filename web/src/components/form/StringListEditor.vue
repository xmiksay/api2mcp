<script setup lang="ts">
// Editable string list — origin_allowlist, scopes, tags: every place a pack field is `string[]`
// (or a `BTreeSet<String>` on the wire, which serializes identically). Blank rows are dropped on
// emit rather than kept as placeholders, so a half-filled row never survives into the saved def.
import { fieldClass, ghostButtonClass, smallIconButtonClass } from "@/lib/formStyle";

const props = withDefaults(defineProps<{ modelValue: string[]; placeholder?: string; mono?: boolean }>(), {
  placeholder: "",
  mono: false,
});
const emit = defineEmits<{ "update:modelValue": [string[]] }>();

function set(i: number, value: string): void {
  const next = [...props.modelValue];
  next[i] = value;
  emit("update:modelValue", next);
}

function add(): void {
  emit("update:modelValue", [...props.modelValue, ""]);
}

function removeAt(i: number): void {
  emit(
    "update:modelValue",
    props.modelValue.filter((_, idx) => idx !== i),
  );
}
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <div v-for="(value, i) in modelValue" :key="i" class="flex items-center gap-2">
      <input
        :value="value"
        :placeholder="placeholder"
        :class="[fieldClass, mono ? 'font-mono' : '']"
        @input="set(i, ($event.target as HTMLInputElement).value)"
      />
      <button type="button" :class="smallIconButtonClass" title="remove" @click="removeAt(i)">&times;</button>
    </div>
    <button type="button" :class="ghostButtonClass + ' self-start'" @click="add">+ add</button>
  </div>
</template>
