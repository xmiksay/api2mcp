<script setup lang="ts">
// alias -> api_call slug — the full set of HTTP calls a script's `api()`/`api_many()` can ever
// address (I1). Kept as an array of pairs while editing for the same reason KeyValueEditor is:
// an in-progress alias shouldn't collapse rows before it has a name.
import { ref, watch } from "vue";
import type { ApiCallView } from "@/api";
import { fieldClass, ghostButtonClass, monoFieldClass, smallIconButtonClass } from "@/lib/formStyle";

const props = defineProps<{ modelValue: Record<string, string>; apiCalls: ApiCallView[] }>();
const emit = defineEmits<{ "update:modelValue": [Record<string, string>] }>();

interface Pair {
  alias: string;
  target: string;
}

const pairs = ref<Pair[]>(Object.entries(props.modelValue).map(([alias, target]) => ({ alias, target })));

watch(
  () => props.modelValue,
  (next) => {
    const flattened = Object.fromEntries(pairs.value.map((p) => [p.alias, p.target]));
    if (JSON.stringify(flattened) !== JSON.stringify(next)) {
      pairs.value = Object.entries(next).map(([alias, target]) => ({ alias, target }));
    }
  },
);

function commit(): void {
  emit(
    "update:modelValue",
    Object.fromEntries(pairs.value.filter((p) => p.alias.length > 0 && p.target.length > 0).map((p) => [p.alias, p.target])),
  );
}

function add(): void {
  pairs.value = [...pairs.value, { alias: "", target: props.apiCalls[0]?.slug ?? "" }];
}

function removeAt(i: number): void {
  pairs.value = pairs.value.filter((_, idx) => idx !== i);
  commit();
}
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <p v-if="apiCalls.length === 0" class="text-xs text-ink-faint italic">
      no api_calls exist yet — create one before wiring a script to call it
    </p>
    <div v-for="(pair, i) in pairs" :key="i" class="flex items-center gap-2">
      <input
        v-model="pair.alias"
        placeholder="alias used inside api()"
        :class="monoFieldClass + ' max-w-56'"
        @blur="commit"
      />
      <span class="text-ink-faint">&rarr;</span>
      <select v-model="pair.target" :class="fieldClass" @change="commit">
        <option v-for="c in apiCalls" :key="c.slug" :value="c.slug">{{ c.slug }}</option>
      </select>
      <button type="button" :class="smallIconButtonClass" title="remove" @click="removeAt(i)">&times;</button>
    </div>
    <button type="button" :disabled="apiCalls.length === 0" :class="ghostButtonClass + ' self-start'" @click="add">
      + add
    </button>
  </div>
</template>
