<script setup lang="ts">
// Ceiling vs. actual usage for one run. A bare "18" next to a bare "20" printed somewhere else
// makes the reader do the division; this renders the fraction directly so a near-exhausted run
// is visually distinct from one that barely touched its budget at a glance.
import { computed } from "vue";
import type { RunBudgetSnapshot, RunTimings } from "@/api";
import { formatBytes, formatMillis } from "@/lib/format";

const props = defineProps<{ snapshot: RunBudgetSnapshot | null; timings: RunTimings | null }>();

type BarTone = "ok" | "budget" | "error" | "neutral";

// Static class strings, not a dynamic `bg-${tone}` template — Tailwind's build-time scanner only
// picks up class names it can see literally in source.
const BAR_CLASSES: Record<BarTone, string> = {
  ok: "bg-ok",
  budget: "bg-budget",
  error: "bg-error",
  neutral: "bg-border-strong",
};

interface UsageRow {
  label: string;
  detail: string;
  percent: number | null;
  tone: BarTone;
}

function toneFor(percent: number): BarTone {
  if (percent >= 90) return "error";
  if (percent >= 70) return "budget";
  return "ok";
}

function usageRow(label: string, used: number, max: number | null, format: (n: number) => string): UsageRow {
  if (max === null) return { label, detail: `${format(used)} used (no ceiling)`, percent: null, tone: "neutral" };
  const percent = max > 0 ? Math.min(100, (used / max) * 100) : used > 0 ? 100 : 0;
  return { label, detail: `${format(used)} / ${format(max)}`, percent, tone: toneFor(percent) };
}

const rows = computed<UsageRow[]>(() => {
  const s = props.snapshot;
  if (!s) return [];
  const out = [
    usageRow("calls", s.calls_used, s.max_calls, (n) => String(n)),
    usageRow("bytes in", s.bytes_used, s.max_bytes, (n) => formatBytes(n)),
    usageRow("pages fetched", s.pages_used, s.max_pages, (n) => String(n)),
  ];
  // The meter never records "wall clock used" the way it does the other three axes (see
  // `BudgetMeter::snapshot_json`) — the run's own elapsed timing, when recorded, is the closest
  // stand-in; otherwise the ceiling is worth showing on its own rather than not at all.
  if (s.wall_clock_ms !== null) {
    const elapsed = props.timings?.elapsed_ms;
    out.push(
      elapsed !== undefined
        ? usageRow("wall clock", elapsed, s.wall_clock_ms, (n) => formatMillis(n))
        : { label: "wall clock", detail: `ceiling ${formatMillis(s.wall_clock_ms)}`, percent: null, tone: "neutral" },
    );
  }
  return out;
});
</script>

<template>
  <p v-if="rows.length === 0" class="text-xs text-ink-faint italic">no budget snapshot recorded</p>
  <dl v-else class="flex flex-col gap-2.5">
    <div v-for="r in rows" :key="r.label" class="flex items-center gap-3">
      <dt class="w-28 shrink-0 text-[11px] tracking-wide text-ink-faint uppercase">{{ r.label }}</dt>
      <div class="h-1.5 flex-1 bg-border-strong">
        <div v-if="r.percent !== null" class="h-full" :class="BAR_CLASSES[r.tone]" :style="{ width: `${r.percent}%` }" />
      </div>
      <dd class="w-40 shrink-0 text-right text-xs text-ink-dim">{{ r.detail }}</dd>
    </div>
  </dl>
</template>
