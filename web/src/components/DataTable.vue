<script setup lang="ts" generic="T extends object">
// A plain data table: columns declare what to show, the parent supplies a per-column scoped
// slot (`#cell-<key>`) when a raw stringified value isn't enough (status pills, links, tags).
// Loading/empty/error states are deliberately NOT handled here — every list view wraps this with
// its own `LoadingState`/`EmptyState`/`ErrorState` around the store's own flags, so this stays a
// pure renderer reusable for definitions, runs, or anything else with rows and columns.
//
// `T extends object` (not `Record<string, unknown>`) so a caller can pass a concrete DTO
// interface as-is — TS interfaces without their own index signature aren't structurally
// assignable to an indexed type, even when every property matches. The dynamic `row[key]` lookup
// this needs is confined to `cell()` below instead.
import type { Column } from "@/lib/table";

defineProps<{
  columns: Column[];
  rows: T[];
  rowKey: (row: T) => string;
}>();

const emit = defineEmits<{ rowClick: [row: T] }>();

function cell(row: T, key: string): unknown {
  return (row as Record<string, unknown>)[key];
}
</script>

<template>
  <div class="overflow-x-auto border border-border">
    <table class="w-full border-collapse text-left text-sm">
      <thead>
        <tr class="border-b border-border bg-surface-raised">
          <th
            v-for="col in columns"
            :key="col.key"
            class="px-3 py-2 text-[11px] font-medium tracking-[0.1em] text-ink-faint uppercase"
            :class="col.class"
          >
            {{ col.label }}
          </th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="row in rows"
          :key="rowKey(row)"
          class="cursor-pointer border-b border-border last:border-b-0 hover:bg-surface-raised"
          @click="emit('rowClick', row)"
        >
          <td v-for="col in columns" :key="col.key" class="px-3 py-2 align-top" :class="col.class">
            <slot :name="`cell-${col.key}`" :row="row" :value="cell(row, col.key)">
              {{ cell(row, col.key) }}
            </slot>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
