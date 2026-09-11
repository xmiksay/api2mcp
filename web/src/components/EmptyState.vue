<script setup lang="ts">
withDefaults(defineProps<{ title?: string; hint?: string }>(), {
  title: "nothing here",
  hint: undefined,
});
</script>

<template>
  <div class="flex flex-col items-center gap-2 border border-dashed border-border px-6 py-14 text-center">
    <div class="text-sm tracking-[0.15em] text-ink-dim uppercase">{{ title }}</div>
    <p v-if="hint" class="max-w-md text-xs text-ink-faint">{{ hint }}</p>
    <div v-if="$slots.default" class="mt-3">
      <slot />
    </div>
  </div>
</template>
