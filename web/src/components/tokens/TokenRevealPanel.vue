<script setup lang="ts">
// The plaintext lives here and nowhere else: it arrives as a prop from the create call, is never
// written to a store/localStorage/URL, and `dismiss` (gated on an explicit checkbox, not just a
// click) is the caller's cue to drop its own reference too. Once this component unmounts, the
// value is gone — there is no way back to it from anywhere else in the app.
import { ref } from "vue";
import type { TokenCreateResponse } from "@/api";
import { useClipboard } from "@/composables/useClipboard";
import TokenUsageHint from "./TokenUsageHint.vue";
import { checkboxClass, monoFieldClass, primaryButtonClass } from "@/lib/formStyle";

defineProps<{ token: TokenCreateResponse; allSlugs: string[] }>();
const emit = defineEmits<{ dismiss: [] }>();

const { copied, copy } = useClipboard();
const acknowledged = ref(false);

function selectAll(ev: FocusEvent): void {
  (ev.target as HTMLInputElement).select();
}
</script>

<template>
  <section class="mb-6 border border-write/50 bg-write/5 p-4">
    <h2 class="mb-1 text-sm font-semibold tracking-wide text-write uppercase">token generated — shown once</h2>
    <p class="mb-3 max-w-2xl text-xs text-ink-dim">
      Copy <span class="font-mono text-ink">{{ token.label }}</span> now. This is the only time the full value is
      shown — api2mcp keeps only its prefix (<span class="font-mono text-ink">{{ token.token_prefix }}</span
      >) from here on, and it cannot be recovered if lost.
    </p>

    <div class="mb-4 flex items-center gap-2">
      <input
        :value="token.token"
        readonly
        :class="monoFieldClass + ' flex-1'"
        @focus="selectAll"
      />
      <button type="button" :class="primaryButtonClass" @click="copy(token.token)">
        {{ copied ? "copied" : "copy" }}
      </button>
    </div>

    <p class="mb-2 text-[11px] tracking-[0.1em] text-ink-faint uppercase">point an mcp client at it</p>
    <TokenUsageHint :endpoints="token.endpoints" :all-slugs="allSlugs" :token="token.token" />

    <label class="mt-4 flex items-center gap-2 text-sm text-ink">
      <input v-model="acknowledged" type="checkbox" :class="checkboxClass" />
      I have saved this token — it will not be shown again
    </label>
    <button
      type="button"
      :disabled="!acknowledged"
      :class="primaryButtonClass + ' mt-3'"
      @click="emit('dismiss')"
    >
      done
    </button>
  </section>
</template>
