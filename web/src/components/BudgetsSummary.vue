<script setup lang="ts">
// Renders a `PackBudgets` (optional fields) or a `BudgetsView` (nullable fields) — both carry the
// same five axes, just with a different "unset" spelling depending on which Rust type wrote them.
import { computed } from "vue";
import { formatBytes, formatMillis } from "@/lib/format";

const props = defineProps<{
  budgets: {
    max_calls?: number | null;
    max_bytes?: number | null;
    wall_clock_ms?: number | null;
    max_pages?: number | null;
    max_concurrency?: number | null;
  };
}>();

const rows = computed(() =>
  [
    { label: "max calls", value: props.budgets.max_calls != null ? String(props.budgets.max_calls) : null },
    { label: "max bytes", value: formatBytes(props.budgets.max_bytes ?? undefined) === "—" ? null : formatBytes(props.budgets.max_bytes) },
    { label: "wall clock", value: props.budgets.wall_clock_ms != null ? formatMillis(props.budgets.wall_clock_ms) : null },
    { label: "max pages", value: props.budgets.max_pages != null ? String(props.budgets.max_pages) : null },
    { label: "max concurrency", value: props.budgets.max_concurrency != null ? String(props.budgets.max_concurrency) : null },
  ].filter((r) => r.value !== null),
);
</script>

<template>
  <p v-if="rows.length === 0" class="text-xs text-ink-faint italic">no budget ceiling set</p>
  <dl v-else class="flex flex-wrap gap-x-6 gap-y-1">
    <div v-for="row in rows" :key="row.label" class="flex items-baseline gap-1.5">
      <dt class="text-[11px] tracking-wide text-ink-faint uppercase">{{ row.label }}</dt>
      <dd class="text-sm text-ink">{{ row.value }}</dd>
    </div>
  </dl>
</template>
