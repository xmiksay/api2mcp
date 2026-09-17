<script setup lang="ts">
// One label/value row in a detail view's field list. Used across every *DetailView — extracted
// once the third detail view wanted the identical `<dt>/<dd>` pair.
withDefaults(defineProps<{ label: string; mono?: boolean }>(), { mono: false });
</script>

<template>
  <div class="flex flex-col gap-0.5 border-b border-border py-2 last:border-b-0 sm:flex-row sm:gap-4">
    <dt class="w-40 shrink-0 text-[11px] tracking-[0.1em] text-ink-faint uppercase">{{ label }}</dt>
    <dd class="min-w-0 flex-1 text-sm text-ink" :class="mono ? 'font-mono' : ''">
      <slot />
    </dd>
  </div>
</template>
