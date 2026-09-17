<script setup lang="ts">
withDefaults(defineProps<{ title: string; subtitle?: string; back?: { to: string; label: string } }>(), {
  subtitle: undefined,
  back: undefined,
});
</script>

<template>
  <div class="mb-6 flex items-start justify-between gap-4 border-b border-border pb-4">
    <div>
      <RouterLink
        v-if="back"
        :to="back.to"
        class="mb-1 inline-block text-xs text-ink-faint hover:text-read"
      >
        &larr; {{ back.label }}
      </RouterLink>
      <h1 class="text-lg font-semibold tracking-wide text-ink">{{ title }}</h1>
      <p v-if="subtitle" class="mt-1 text-xs text-ink-dim">{{ subtitle }}</p>
    </div>
    <!-- slot for the next agent's New / Edit / Delete actions -->
    <div class="flex shrink-0 items-center gap-2">
      <slot name="actions" />
    </div>
  </div>
</template>
