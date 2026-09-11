<script setup lang="ts">
// A pretty-printed, copyable JSON block. Kept deliberately simple — no syntax-highlighting
// dependency — with a collapse toggle only for bodies long enough to dominate the screen.
import { computed, ref } from "vue";
import { useClipboard } from "@/composables/useClipboard";

const props = withDefaults(
  defineProps<{ value: unknown; collapseAfterLines?: number; nullLabel?: string }>(),
  { collapseAfterLines: 24, nullLabel: "null" },
);

const text = computed(() => {
  if (props.value === null || props.value === undefined) return null;
  return JSON.stringify(props.value, null, 2);
});
const lineCount = computed(() => (text.value ? text.value.split("\n").length : 0));
const expanded = ref(false);
const { copied, copy: copyText } = useClipboard();

function copy(): void {
  if (text.value) void copyText(text.value);
}
</script>

<template>
  <div v-if="text === null" class="text-xs text-ink-faint italic">{{ nullLabel }}</div>
  <div v-else class="relative border border-border bg-surface">
    <div class="flex items-center justify-between border-b border-border px-2 py-1">
      <span class="text-[10px] tracking-[0.15em] text-ink-faint uppercase">json</span>
      <button
        type="button"
        class="text-[10px] tracking-wide text-ink-dim uppercase hover:text-read"
        @click="copy"
      >
        {{ copied ? "copied" : "copy" }}
      </button>
    </div>
    <pre
      class="overflow-x-auto p-2 text-xs leading-relaxed text-ink"
      :class="!expanded && lineCount > collapseAfterLines ? 'max-h-64 overflow-y-hidden' : ''"
    >{{ text }}</pre>
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
