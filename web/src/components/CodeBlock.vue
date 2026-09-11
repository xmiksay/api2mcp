<script setup lang="ts">
// Plain-text source display (Rhai script bodies) — JsonViewer's layout without the JSON-specific
// bits. A script's source is untrusted content in the sense `pack::mod`'s doc means (review it
// before trusting it), not something to syntax-highlight here.
import { computed, ref } from "vue";
import { useClipboard } from "@/composables/useClipboard";

const props = withDefaults(defineProps<{ code: string; collapseAfterLines?: number }>(), {
  collapseAfterLines: 24,
});

const lineCount = computed(() => props.code.split("\n").length);
const expanded = ref(false);
const { copied, copy: copyText } = useClipboard();
</script>

<template>
  <div class="relative border border-border bg-surface">
    <div class="flex items-center justify-between border-b border-border px-2 py-1">
      <span class="text-[10px] tracking-[0.15em] text-ink-faint uppercase">rhai</span>
      <button
        type="button"
        class="text-[10px] tracking-wide text-ink-dim uppercase hover:text-read"
        @click="copyText(code)"
      >
        {{ copied ? "copied" : "copy" }}
      </button>
    </div>
    <pre
      class="overflow-x-auto p-2 text-xs leading-relaxed text-ink"
      :class="!expanded && lineCount > collapseAfterLines ? 'max-h-80 overflow-y-hidden' : ''"
    >{{ code }}</pre>
    <button
      v-if="lineCount > collapseAfterLines"
      type="button"
      class="w-full border-t border-border py-1 text-[10px] tracking-wide text-ink-dim uppercase hover:text-read"
      @click="expanded = !expanded"
    >
      {{ expanded ? "collapse" : `show all ${lineCount} lines` }}
    </button>
  </div>
</template>
