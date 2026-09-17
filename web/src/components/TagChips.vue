<script setup lang="ts">
withDefaults(defineProps<{ tags: string[] }>(), { tags: () => [] });
</script>

<template>
  <span v-if="tags.length === 0" class="text-xs text-ink-faint">—</span>
  <span v-else class="flex flex-wrap gap-1">
    <span
      v-for="tag in tags"
      :key="tag"
      class="border border-border-strong px-1.5 py-0.5 text-[11px] text-ink-dim"
    >{{ tag }}</span>
  </span>
</template>
