<script setup lang="ts">
// Add/remove/reorder shell around ParamRow. `position` is never hand-edited — it's derived from
// array order on every emit, so reordering (the up/down buttons) is the only way to change it,
// and two params can never collide on the same position by typo.
import { computed } from "vue";
import type { PackParam } from "@/api";
import { ghostButtonClass, smallIconButtonClass } from "@/lib/formStyle";
import type { LocationKind } from "@/lib/paramLocation";
import ParamRow from "./ParamRow.vue";

const props = defineProps<{ modelValue: PackParam[]; allowedLocations: LocationKind[] }>();
const emit = defineEmits<{ "update:modelValue": [PackParam[]] }>();

const sorted = computed(() => [...props.modelValue].sort((a, b) => a.position - b.position));

function renumber(list: PackParam[]): PackParam[] {
  return list.map((p, i) => ({ ...p, position: i }));
}

function updateAt(i: number, param: PackParam): void {
  const next = [...sorted.value];
  next[i] = param;
  emit("update:modelValue", renumber(next));
}

function removeAt(i: number): void {
  emit("update:modelValue", renumber(sorted.value.filter((_, idx) => idx !== i)));
}

function move(i: number, delta: number): void {
  const j = i + delta;
  if (j < 0 || j >= sorted.value.length) return;
  const next = [...sorted.value];
  [next[i], next[j]] = [next[j], next[i]];
  emit("update:modelValue", renumber(next));
}

function add(): void {
  const defaultLocation = props.allowedLocations[0] ?? "query";
  emit(
    "update:modelValue",
    renumber([
      ...sorted.value,
      {
        name: "",
        location: defaultLocation === "body" ? { body: "" } : defaultLocation,
        type: "string",
        required: false,
        position: sorted.value.length,
      },
    ]),
  );
}
</script>

<template>
  <div class="flex flex-col gap-3">
    <p v-if="sorted.length === 0" class="text-xs text-ink-faint italic">no parameters</p>
    <div v-for="(p, i) in sorted" :key="i" class="flex items-start gap-2">
      <div class="flex shrink-0 flex-col gap-1 pt-1">
        <button type="button" :disabled="i === 0" :class="smallIconButtonClass" title="move up" @click="move(i, -1)">&uarr;</button>
        <button
          type="button"
          :disabled="i === sorted.length - 1"
          :class="smallIconButtonClass"
          title="move down"
          @click="move(i, 1)"
        >&darr;</button>
      </div>
      <div class="min-w-0 flex-1">
        <ParamRow
          :param="p"
          :allowed-locations="allowedLocations"
          @update:param="(next) => updateAt(i, next)"
          @remove="removeAt(i)"
        />
      </div>
    </div>
    <button type="button" :class="ghostButtonClass + ' self-start'" @click="add">+ add param</button>
  </div>
</template>
