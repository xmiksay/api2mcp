<script setup lang="ts">
// The per-call breakdown for a script test run — `ScriptCallView` (test_run.rs) is a narrower
// shape than `RunCallView` (no `response_truncated`, a typed `error` object rather than a string), so this is its own small renderer
// rather than a forced fit into `RunCallTimeline`. Rendered in input order (`seq`), never re-sorted.
// `raw` is absent unless the test ran with `include_raw`, so the body toggle only appears then.
import { ref } from "vue";
import type { ScriptCallView } from "@/api";
import { formatBytes } from "@/lib/format";
import JsonViewer from "@/components/JsonViewer.vue";
import StatusPill from "@/components/StatusPill.vue";

defineProps<{ calls: ScriptCallView[] }>();

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
      <button type="button" class="flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-surface-raised" @click="call.raw !== undefined && toggle(call.seq)">
        <span class="w-8 shrink-0 text-xs text-ink-faint">#{{ call.seq }}</span>
        <span class="shrink-0 font-medium text-ink">{{ call.api_call_slug }}</span>
        <span class="shrink-0 text-xs text-ink-faint">via {{ call.service_slug }}</span>
        <StatusPill :label="call.status_code !== null ? String(call.status_code) : 'no response'" :tone="statusTone(call.status_code)" />
        <span class="text-xs text-ink-dim">{{ formatBytes(call.response_bytes) }}</span>
        <span v-if="call.raw !== undefined" class="ml-auto shrink-0 text-xs text-ink-faint">{{ expanded.has(call.seq) ? "hide" : "show" }} body</span>
      </button>
      <p v-if="call.error" class="border-t border-border px-3 py-2 text-xs text-ink-dim">
        <span class="font-medium">{{ call.error.kind }}</span>: {{ call.error.message }}
      </p>
      <div v-if="call.raw !== undefined && expanded.has(call.seq)" class="border-t border-border p-3">
        <JsonViewer :value="call.raw" null-label="no response body recorded" />
      </div>
    </li>
  </ol>
  <p v-else class="text-xs text-ink-faint italic">no upstream calls were made</p>
</template>
