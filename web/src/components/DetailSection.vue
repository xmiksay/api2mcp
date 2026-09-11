<script setup lang="ts">
defineProps<{ title: string }>();
</script>

<template>
  <section class="mb-6 border border-border bg-surface">
    <h2 class="border-b border-border px-4 py-2 text-[11px] tracking-[0.15em] text-ink-faint uppercase">
      {{ title }}
    </h2>
    <div class="p-4">
      <slot />
    </div>
  </section>
</template>
