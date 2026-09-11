<script setup lang="ts">
// One label+control+help+error group, shared by every write form — the editable counterpart to
// FieldRow's read-only label/value pair.
withDefaults(
  defineProps<{ label: string; help?: string; errors?: string[]; required?: boolean }>(),
  { help: undefined, errors: () => [], required: false },
);
</script>

<template>
  <div class="flex flex-col gap-1">
    <label class="text-[11px] tracking-[0.1em] text-ink-faint uppercase">
      {{ label }}<span v-if="required" class="text-write">&nbsp;*</span>
    </label>
    <slot />
    <p v-if="help" class="text-xs text-ink-faint">{{ help }}</p>
    <p v-for="(e, i) in errors" :key="i" class="text-xs text-error">{{ e }}</p>
  </div>
</template>
