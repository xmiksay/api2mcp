<script setup lang="ts">
// `PackPagination` is internally tagged on `kind`: `{kind:"none"}` or
// `{kind:"cursor", next_cursor_path, query_param}` — a radio between the two shapes, not a
// checkbox, since "cursor" always carries its own two fields.
import type { PackPagination } from "@/api";
import { monoFieldClass } from "@/lib/formStyle";

const props = defineProps<{ modelValue: PackPagination }>();
const emit = defineEmits<{ "update:modelValue": [PackPagination] }>();

function setKind(kind: "none" | "cursor"): void {
  emit(
    "update:modelValue",
    kind === "none" ? { kind: "none" } : { kind: "cursor", next_cursor_path: "", query_param: "" },
  );
}

function setField(field: "next_cursor_path" | "query_param", value: string): void {
  if (props.modelValue.kind !== "cursor") return;
  emit("update:modelValue", { ...props.modelValue, [field]: value });
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <div class="flex gap-4 text-sm text-ink-dim">
      <label class="flex items-center gap-1.5">
        <input type="radio" :checked="modelValue.kind === 'none'" @change="setKind('none')" />
        none
      </label>
      <label class="flex items-center gap-1.5">
        <input type="radio" :checked="modelValue.kind === 'cursor'" @change="setKind('cursor')" />
        cursor
      </label>
    </div>
    <div v-if="modelValue.kind === 'cursor'" class="flex flex-wrap items-center gap-2">
      <input
        :value="modelValue.next_cursor_path"
        placeholder="/json/pointer/to/next_cursor"
        :class="monoFieldClass + ' max-w-64'"
        @input="setField('next_cursor_path', ($event.target as HTMLInputElement).value)"
      />
      <input
        :value="modelValue.query_param"
        placeholder="query param name the cursor is sent as"
        :class="monoFieldClass + ' max-w-64'"
        @input="setField('query_param', ($event.target as HTMLInputElement).value)"
      />
    </div>
  </div>
</template>
