<script setup lang="ts">
withDefaults(defineProps<{ label?: string }>(), { label: "loading" });
</script>

<template>
  <div class="flex items-center gap-3 px-4 py-10 text-ink-faint">
    <span class="inline-block h-3 w-3 animate-spin border border-current border-t-transparent" />
    <span class="text-xs tracking-[0.2em] uppercase">{{ label }}&hellip;</span>
  </div>
</template>
