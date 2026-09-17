<script setup lang="ts">
// Live `inputSchema` preview — recomputed on every param edit so the author sees exactly what a
// model will be handed, not just what ParamTable says about the saved definition. See
// `lib/inputSchema.ts` for why this is a client-side mirror of `schema::input_schema` rather than
// a round trip to the server on every keystroke.
import { computed } from "vue";
import type { PackParam } from "@/api";
import { buildInputSchema } from "@/lib/inputSchema";
import JsonViewer from "@/components/JsonViewer.vue";

const props = defineProps<{ params: PackParam[] }>();
const schema = computed(() => buildInputSchema(props.params));
</script>

<template>
  <JsonViewer :value="schema" />
</template>
