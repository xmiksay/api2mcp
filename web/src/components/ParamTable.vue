<script setup lang="ts">
// The param table an api_call or script detail view shows to make curation visible: which
// params a caller can set, and which are `fixed` — set by the definer, invisible to the model
// (`Param::is_model_visible` on the Rust side; `input_schema` skips a fixed param entirely).
import { computed } from "vue";
import type { PackParam, ParamLocation } from "@/api";

const props = defineProps<{ params: PackParam[] }>();

const sorted = computed(() => [...props.params].sort((a, b) => a.position - b.position));

function locationLabel(loc: ParamLocation): string {
  if (typeof loc === "string") return loc;
  return `body:${loc.body}`;
}

function jsonOrDash(v: unknown): string {
  return v === undefined ? "—" : JSON.stringify(v);
}
</script>

<template>
  <p v-if="params.length === 0" class="text-xs text-ink-faint italic">no parameters</p>
  <div v-else class="overflow-x-auto border border-border">
    <table class="w-full border-collapse text-left text-sm">
      <thead>
        <tr class="border-b border-border bg-surface-raised">
          <th class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">name</th>
          <th class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">source</th>
          <th class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">location</th>
          <th class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">type</th>
          <th class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">required</th>
          <th class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">value</th>
          <th class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">description</th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="p in sorted"
          :key="p.name"
          class="border-b border-border last:border-b-0"
          :class="p.fixed !== undefined ? 'bg-write/5' : ''"
        >
          <td class="px-3 py-2 font-medium text-ink">{{ p.name }}</td>
          <td class="px-3 py-2">
            <span
              class="border px-1.5 py-0.5 text-[10px] tracking-wide uppercase"
              :class="p.fixed !== undefined ? 'border-write/40 text-write' : 'border-read/40 text-read'"
            >
              {{ p.fixed !== undefined ? "fixed" : "caller" }}
            </span>
          </td>
          <td class="px-3 py-2 text-ink-dim">{{ locationLabel(p.location) }}</td>
          <td class="px-3 py-2 text-ink-dim">{{ p.type }}</td>
          <td class="px-3 py-2 text-ink-dim">{{ p.required ? "yes" : "no" }}</td>
          <td class="px-3 py-2 font-mono text-xs text-ink-dim">
            {{ p.fixed !== undefined ? jsonOrDash(p.fixed) : jsonOrDash(p.default) }}
            <span v-if="p.enum_values?.length" class="block text-ink-faint">
              one of {{ p.enum_values.map((v) => JSON.stringify(v)).join(", ") }}
            </span>
          </td>
          <td class="px-3 py-2 text-ink-dim">{{ p.description ?? "—" }}</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
