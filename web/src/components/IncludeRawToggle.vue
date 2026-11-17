<script setup lang="ts">
// Off by default: a test run's raw upstream bodies can be megabytes for a small result, so the
// test routes only return them on request (`include_raw`).
import { checkboxClass } from "@/lib/formStyle";

const includeRaw = defineModel<boolean>({ required: true });
</script>

<template>
  <label class="flex items-center gap-2 text-xs text-ink-dim">
    <input v-model="includeRaw" type="checkbox" :class="checkboxClass" />
    include raw upstream response bodies
  </label>
</template>
