<script setup lang="ts">
// Editable string->string map — `default_headers`, `query_fixed`. Kept as a local array of pairs
// while editing (not a live `Record`) so typing a duplicate or half-finished key doesn't collapse
// two rows into one before the user is done; the `Record` is only rebuilt on emit.
import { ref, watch } from "vue";
import { fieldClass, ghostButtonClass, smallIconButtonClass } from "@/lib/formStyle";

const props = defineProps<{ modelValue: Record<string, string> }>();
const emit = defineEmits<{ "update:modelValue": [Record<string, string>] }>();

interface Pair {
  key: string;
  value: string;
}

const pairs = ref<Pair[]>(Object.entries(props.modelValue).map(([key, value]) => ({ key, value })));

// The parent can replace `modelValue` wholesale (e.g. loading a definition into the form) after
// this component has already mounted with an empty default — resync when that happens.
watch(
  () => props.modelValue,
  (next) => {
    const asPairs = Object.entries(next);
    const flattened = Object.fromEntries(pairs.value.map((p) => [p.key, p.value]));
    if (JSON.stringify(flattened) !== JSON.stringify(next)) {
      pairs.value = asPairs.map(([key, value]) => ({ key, value }));
    }
  },
);

function commit(): void {
  emit("update:modelValue", Object.fromEntries(pairs.value.filter((p) => p.key.length > 0).map((p) => [p.key, p.value])));
}

function add(): void {
  pairs.value = [...pairs.value, { key: "", value: "" }];
}

function removeAt(i: number): void {
  pairs.value = pairs.value.filter((_, idx) => idx !== i);
  commit();
}
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <div v-for="(pair, i) in pairs" :key="i" class="flex items-center gap-2">
      <input v-model="pair.key" placeholder="name" :class="fieldClass + ' font-mono'" @blur="commit" />
      <span class="text-ink-faint">=</span>
      <input v-model="pair.value" placeholder="value" :class="fieldClass + ' font-mono'" @blur="commit" />
      <button type="button" :class="smallIconButtonClass" title="remove" @click="removeAt(i)">&times;</button>
    </div>
    <button type="button" :class="ghostButtonClass + ' self-start'" @click="add">+ add</button>
  </div>
</template>
