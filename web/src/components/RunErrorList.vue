<script setup lang="ts">
// The run-level `errors[]` envelope — one row per failed batch item, keyed by its input index.
// Not `DataTable`: these rows aren't a navigation target, so this skips its hover/click affordance
// rather than wire an unused `rowClick` that would make the rows look clickable.
import type { RunErrorEntry } from "@/api";

defineProps<{ errors: RunErrorEntry[] }>();
</script>

<template>
  <div class="overflow-x-auto border border-border">
    <table class="w-full border-collapse text-left text-sm">
      <thead>
        <tr class="border-b border-border bg-surface-raised">
          <th class="w-16 px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">index</th>
          <th class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">item</th>
          <th class="w-32 px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">kind</th>
          <th class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase">error</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="e in errors" :key="e.index" class="border-b border-border last:border-b-0">
          <td class="px-3 py-2 align-top font-mono text-xs text-ink-dim">{{ e.index }}</td>
          <td class="px-3 py-2 align-top font-mono text-xs text-ink">{{ e.name }}</td>
          <td class="px-3 py-2 align-top">
            <span class="font-mono text-[11px] tracking-wide text-error uppercase">{{ e.error.kind }}</span>
          </td>
          <td class="px-3 py-2 align-top text-xs text-error">{{ e.error.message }}</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
