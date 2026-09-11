<script setup lang="ts">
import { computed } from "vue";
import type { Tone } from "@/lib/tone";
import { toneClasses } from "@/lib/tone";

const props = defineProps<{ label: string; tone: Tone }>();
const classes = computed(() => toneClasses(props.tone));
</script>

<template>
  <span
    class="inline-flex items-center gap-1.5 border px-1.5 py-0.5 text-[11px] font-medium tracking-wide uppercase"
    :class="classes"
  >
    <span class="h-1.5 w-1.5 rounded-full bg-current" />
    {{ label }}
  </span>
</template>
