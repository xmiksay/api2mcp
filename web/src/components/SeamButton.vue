<script setup lang="ts">
// A disabled placeholder for the write actions the next agent adds (New / Edit / Delete / Test).
// Visibly present — so the layout it lands in is already right — but inert, per the brief's "a
// disabled button ... not a half-built form" instruction.
withDefaults(defineProps<{ label: string }>(), {});
</script>

<template>
  <button
    type="button"
    disabled
    :title="`${label} — not implemented in this chunk`"
    class="cursor-not-allowed border border-border-strong px-3 py-1.5 text-xs tracking-wide text-ink-faint uppercase opacity-50"
  >
    {{ label }}
  </button>
</template>
