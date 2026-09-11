<script setup lang="ts">
// Editable projection field list — the reshaping a caller actually sees instead of the raw
// upstream body. `coerce` is optional (defaults to whatever the JSONPath match already is).
import type { Cardinality, PackProjectionField, ParamType } from "@/api";
import { fieldClass, ghostButtonClass, monoFieldClass, smallIconButtonClass } from "@/lib/formStyle";

const props = defineProps<{ modelValue: PackProjectionField[] }>();
const emit = defineEmits<{ "update:modelValue": [PackProjectionField[]] }>();

const CARDINALITIES: Cardinality[] = ["one", "many"];
const COERCE_TYPES: ParamType[] = ["string", "integer", "number", "boolean", "string_array"];

function updateAt(i: number, field: PackProjectionField): void {
  const next = [...props.modelValue];
  next[i] = field;
  emit("update:modelValue", next);
}

function removeAt(i: number): void {
  emit("update:modelValue", props.modelValue.filter((_, idx) => idx !== i));
}

function add(): void {
  emit("update:modelValue", [...props.modelValue, { name: "", path: "$", cardinality: "one" }]);
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <p v-if="modelValue.length === 0" class="text-xs text-ink-faint italic">
      no fields — the raw upstream response would pass through unmodified
    </p>
    <div v-for="(f, i) in modelValue" :key="i" class="flex flex-wrap items-center gap-2">
      <input
        :value="f.name"
        placeholder="field name"
        :class="fieldClass + ' max-w-40'"
        @input="updateAt(i, { ...f, name: ($event.target as HTMLInputElement).value })"
      />
      <input
        :value="f.path"
        placeholder="$.json.path"
        :class="monoFieldClass + ' max-w-56'"
        @input="updateAt(i, { ...f, path: ($event.target as HTMLInputElement).value })"
      />
      <select
        :value="f.cardinality"
        :class="fieldClass + ' max-w-28'"
        @change="updateAt(i, { ...f, cardinality: ($event.target as HTMLSelectElement).value as Cardinality })"
      >
        <option v-for="c in CARDINALITIES" :key="c" :value="c">{{ c }}</option>
      </select>
      <select
        :value="f.coerce ?? ''"
        :class="fieldClass + ' max-w-32'"
        @change="updateAt(i, { ...f, coerce: (($event.target as HTMLSelectElement).value || undefined) as ParamType | undefined })"
      >
        <option value="">no coercion</option>
        <option v-for="ty in COERCE_TYPES" :key="ty" :value="ty">{{ ty }}</option>
      </select>
      <button type="button" :class="smallIconButtonClass" title="remove field" @click="removeAt(i)">&times;</button>
    </div>
    <button type="button" :class="ghostButtonClass + ' self-start'" @click="add">+ add field</button>
  </div>
</template>
