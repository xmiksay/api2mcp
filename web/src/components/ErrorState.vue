<script setup lang="ts">
defineProps<{ message: string }>();
const emit = defineEmits<{ retry: [] }>();
</script>

<template>
  <div class="flex items-start justify-between gap-4 border border-error/40 bg-error/10 px-4 py-3">
    <div class="flex items-start gap-2">
      <span class="mt-0.5 text-error">&#9888;</span>
      <p class="text-sm text-ink">{{ message }}</p>
    </div>
    <button
      type="button"
      class="shrink-0 border border-error/50 px-2 py-1 text-xs tracking-wide text-error uppercase hover:bg-error/20"
      @click="emit('retry')"
    >
      retry
    </button>
  </div>
</template>
