<script setup lang="ts">
// A destructive action that arms on first click and fires on the second, rather than a native
// `confirm()` dialog (which can't say *what* will break — see this component's `detail` slot,
// used to surface the server's own rejection reason inline instead of a generic failure).
import { ref } from "vue";
import { dangerButtonClass, ghostButtonClass } from "@/lib/formStyle";

const props = withDefaults(defineProps<{ label: string; pending?: boolean }>(), { pending: false });
const emit = defineEmits<{ confirm: [] }>();

const armed = ref(false);
let disarmTimer: ReturnType<typeof setTimeout> | undefined;

function click(): void {
  if (props.pending) return;
  if (armed.value) {
    armed.value = false;
    emit("confirm");
    return;
  }
  armed.value = true;
  disarmTimer = setTimeout(() => (armed.value = false), 4000);
}

function cancel(): void {
  clearTimeout(disarmTimer);
  armed.value = false;
}
</script>

<template>
  <span class="inline-flex items-center gap-2">
    <button
      type="button"
      :disabled="pending"
      :class="armed ? 'border border-error bg-error/20 px-3 py-1.5 text-xs font-medium tracking-wide text-error uppercase' : dangerButtonClass"
      @click="click"
    >
      {{ pending ? "working…" : armed ? "click again to confirm" : label }}
    </button>
    <button v-if="armed" type="button" :class="ghostButtonClass" @click="cancel">cancel</button>
  </span>
</template>
