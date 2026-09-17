<script setup lang="ts">
// Renders a `ScriptFailure` against the script's own source — line-numbered, the failing line
// highlighted, with the server's own caret snippet directly underneath it. This is the difference
// the brief calls out between a usable editor and a text box: without it, "line 14, column 6" is
// a coordinate the author has to go count out by hand.
import { computed } from "vue";
import type { ScriptFailure } from "@/api";
import StatusPill from "@/components/StatusPill.vue";

const props = defineProps<{ source: string; failure: ScriptFailure }>();

const lines = computed(() => props.source.split("\n"));
/** The `^`-under-the-column line is the snippet's own second line — see `ScriptFailure::build`. */
const caretLine = computed(() => props.failure.snippet?.split("\n")[1] ?? null);

const kindLabel = computed(() => props.failure.kind.replace(/_/g, " "));
</script>

<template>
  <div class="border border-error/40 bg-error/5">
    <div class="flex flex-wrap items-center gap-2 border-b border-error/30 px-3 py-2">
      <StatusPill :label="kindLabel" tone="error" />
      <span v-if="failure.line" class="font-mono text-xs text-ink-faint">
        line {{ failure.line }}<span v-if="failure.column">, column {{ failure.column }}</span>
      </span>
      <span class="text-sm text-ink">{{ failure.message }}</span>
    </div>
    <div v-if="failure.line" class="max-h-80 overflow-auto p-2 font-mono text-xs leading-relaxed">
      <template v-for="(line, i) in lines" :key="i">
        <div v-if="i + 1 === failure.line" class="bg-error/15">
          <span class="inline-block w-10 shrink-0 select-none text-right text-ink-faint">{{ i + 1 }}</span>
          <span class="whitespace-pre text-ink">{{ line }}</span>
        </div>
        <div v-if="i + 1 === failure.line && caretLine !== null" class="bg-error/15 text-error">
          <span class="inline-block w-10 shrink-0"></span>
          <span class="whitespace-pre">{{ caretLine }}</span>
        </div>
        <div v-if="i + 1 !== failure.line" class="text-ink-dim">
          <span class="inline-block w-10 shrink-0 select-none text-right text-ink-faint">{{ i + 1 }}</span>
          <span class="whitespace-pre">{{ line }}</span>
        </div>
      </template>
    </div>
  </div>
</template>
