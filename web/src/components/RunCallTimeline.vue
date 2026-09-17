<script setup lang="ts">
// The per-upstream-call timeline on a run's detail view — the auditability claim made concrete.
// `calls` must render in the order the API already returned it (`run_calls.seq`, the *input*
// index of a fan-out, not completion order — see `store::run`'s own doc) and this component
// never sorts it again.
import { ref } from "vue";
import type { RunCallView } from "@/api";
import { formatBytes } from "@/lib/format";
import JsonViewer from "./JsonViewer.vue";
import StatusPill from "./StatusPill.vue";

defineProps<{ calls: RunCallView[] }>();

const expanded = ref<Set<number>>(new Set());
function toggle(seq: number): void {
  const next = new Set(expanded.value);
  if (next.has(seq)) next.delete(seq);
  else next.add(seq);
  expanded.value = next;
}

function statusTone(code: number | null): "ok" | "error" | "neutral" {
  if (code === null) return "neutral";
  return code < 400 ? "ok" : "error";
}
</script>

<template>
  <ol v-if="calls.length > 0" class="flex flex-col gap-2">
    <li v-for="call in calls" :key="call.seq" class="border border-border">
      <button
        type="button"
        class="flex w-full flex-wrap items-center gap-3 px-3 py-2 text-left hover:bg-surface-raised"
        @click="toggle(call.seq)"
      >
        <span class="w-8 shrink-0 text-xs text-ink-faint">#{{ call.seq }}</span>
        <span class="w-14 shrink-0 font-mono text-[11px] text-ink-faint uppercase">{{ call.method }}</span>
        <span class="shrink-0 font-medium text-ink">{{ call.api_call_slug }}</span>
        <span class="shrink-0 text-xs text-ink-faint">via {{ call.service_slug }}</span>
        <StatusPill
          :label="call.status_code !== null ? String(call.status_code) : 'no response'"
          :tone="statusTone(call.status_code)"
        />
        <span class="text-xs text-ink-dim">{{ formatBytes(call.response_bytes) }}</span>
        <span v-if="call.response_truncated" class="text-xs text-budget">truncated</span>
        <span v-if="call.error" class="truncate text-xs text-error">{{ call.error }}</span>
        <span class="ml-auto shrink-0 text-xs text-ink-faint">{{ expanded.has(call.seq) ? "hide" : "show" }} detail</span>
      </button>
      <div v-if="expanded.has(call.seq)" class="flex flex-col gap-3 border-t border-border p-3">
        <div class="font-mono text-xs break-all text-ink-dim">{{ call.url_redacted }}</div>
        <div>
          <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">request headers</div>
          <JsonViewer :value="call.headers_redacted" null-label="no headers recorded" />
        </div>
        <div>
          <div class="mb-1 text-[10px] tracking-[0.15em] text-ink-faint uppercase">raw response body</div>
          <JsonViewer :value="call.raw" null-label="no response body recorded" />
        </div>
      </div>
    </li>
  </ol>
  <p v-else class="text-xs text-ink-faint italic">no upstream calls were made</p>
</template>
