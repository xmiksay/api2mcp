<script setup lang="ts">
// Renders every failure `pack::validate` found, together — never just the first. That plurality
// is the entire point of `server::api::validate_write` (see its module doc): a person fixing a
// form should see everything wrong with it in one pass, not play whack-a-mole one submit at a
// time. Each message already carries its own location prefix (e.g. "api_calls.foo: ...",
// "services.bar: ..." — see `pack::validate::ValidationError`'s `Display`), so this stays a flat
// list rather than trying to re-parse structure back out of a string.
defineProps<{ errors: string[] }>();
</script>

<template>
  <div v-if="errors.length > 0" class="mb-4 border border-error/40 bg-error/10 px-4 py-3">
    <p class="mb-2 text-[11px] tracking-[0.15em] text-error uppercase">
      {{ errors.length }} problem{{ errors.length === 1 ? "" : "s" }} found
    </p>
    <ul class="flex flex-col gap-1.5">
      <li v-for="(e, i) in errors" :key="i" class="font-mono text-xs text-ink">{{ e }}</li>
    </ul>
  </div>
</template>
