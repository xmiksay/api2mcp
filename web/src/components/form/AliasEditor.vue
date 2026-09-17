<script setup lang="ts">
// Endpoint `aliases`: alias -> `{api_call: slug}` or `{script: slug}` — renames a tool's exposed
// name away from its own slug. The target is externally tagged on the wire, so each row picks a
// kind first (which api_call/script list to offer) and then a slug from that list.
import { ref, watch } from "vue";
import type { ApiCallView, PackEndpointTarget, ScriptView } from "@/api";
import { fieldClass, ghostButtonClass, monoFieldClass, smallIconButtonClass } from "@/lib/formStyle";

const props = defineProps<{
  modelValue: Record<string, PackEndpointTarget>;
  apiCalls: ApiCallView[];
  scripts: ScriptView[];
}>();
const emit = defineEmits<{ "update:modelValue": [Record<string, PackEndpointTarget>] }>();

interface Row {
  alias: string;
  kind: "api_call" | "script";
  slug: string;
}

function targetKind(t: PackEndpointTarget): "api_call" | "script" {
  return "api_call" in t ? "api_call" : "script";
}
function targetSlug(t: PackEndpointTarget): string {
  return "api_call" in t ? t.api_call : t.script;
}
function toTarget(kind: "api_call" | "script", slug: string): PackEndpointTarget {
  return kind === "api_call" ? { api_call: slug } : { script: slug };
}

const rows = ref<Row[]>(
  Object.entries(props.modelValue).map(([alias, t]) => ({ alias, kind: targetKind(t), slug: targetSlug(t) })),
);

watch(
  () => props.modelValue,
  (next) => {
    const flattened = Object.fromEntries(rows.value.map((r) => [r.alias, toTarget(r.kind, r.slug)]));
    if (JSON.stringify(flattened) !== JSON.stringify(next)) {
      rows.value = Object.entries(next).map(([alias, t]) => ({ alias, kind: targetKind(t), slug: targetSlug(t) }));
    }
  },
);

function commit(): void {
  emit(
    "update:modelValue",
    Object.fromEntries(
      rows.value.filter((r) => r.alias.length > 0 && r.slug.length > 0).map((r) => [r.alias, toTarget(r.kind, r.slug)]),
    ),
  );
}

function optionsFor(kind: "api_call" | "script"): { slug: string }[] {
  return kind === "api_call" ? props.apiCalls : props.scripts;
}

function add(): void {
  const kind: "api_call" | "script" = props.apiCalls.length > 0 ? "api_call" : "script";
  rows.value = [...rows.value, { alias: "", kind, slug: optionsFor(kind)[0]?.slug ?? "" }];
}

function setKind(i: number, kind: "api_call" | "script"): void {
  rows.value[i] = { ...rows.value[i], kind, slug: optionsFor(kind)[0]?.slug ?? "" };
  commit();
}

function removeAt(i: number): void {
  rows.value = rows.value.filter((_, idx) => idx !== i);
  commit();
}
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <div v-for="(row, i) in rows" :key="i" class="flex items-center gap-2">
      <input v-model="row.alias" placeholder="exposed tool name" :class="monoFieldClass + ' max-w-48'" @blur="commit" />
      <span class="text-ink-faint">&rarr;</span>
      <select :value="row.kind" :class="fieldClass + ' max-w-28'" @change="setKind(i, ($event.target as HTMLSelectElement).value as 'api_call' | 'script')">
        <option value="api_call">api_call</option>
        <option value="script">script</option>
      </select>
      <select v-model="row.slug" :class="fieldClass" @change="commit">
        <option v-for="o in optionsFor(row.kind)" :key="o.slug" :value="o.slug">{{ o.slug }}</option>
      </select>
      <button type="button" :class="smallIconButtonClass" title="remove" @click="removeAt(i)">&times;</button>
    </div>
    <button type="button" :class="ghostButtonClass + ' self-start'" @click="add">+ add alias</button>
  </div>
</template>
