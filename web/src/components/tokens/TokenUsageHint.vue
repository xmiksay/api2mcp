<script setup lang="ts">
// A token is useless without knowing where to point an MCP client, so this renders the exact
// `claude mcp add` line per granted endpoint.
//
// Two forms are valid against this server and they authenticate differently, which is the reason
// `token` is a prop rather than the command being one fixed string:
//
//   * with `--header "Authorization: Bearer <token>"` the client uses the token on this page;
//   * without it, the 401 carries `WWW-Authenticate` pointing at the OAuth discovery document and
//     the client runs the browser flow instead.
//
// On this screen the first form is the right one — showing the OAuth line on the page whose whole
// purpose is handing someone a token would tell them to go and not use it. At reveal time `token`
// is the real value (its lifetime is bounded by the reveal panel, which is also where it is shown);
// afterwards it is a `<token>` placeholder, because the plaintext is unrecoverable by then.
import { computed } from "vue";
import CodeBlock from "@/components/CodeBlock.vue";

const props = withDefaults(
  defineProps<{ endpoints: string[]; allSlugs?: string[]; token?: string }>(),
  { allSlugs: () => [], token: "" },
);

const base = window.location.origin;

// Empty `endpoints` means "every endpoint" per the wire contract, so fall back to every endpoint
// that currently exists to make that concrete rather than abstract.
const slugs = computed<string[]>(() => {
  if (props.endpoints.length > 0) return props.endpoints;
  if (props.allSlugs.length > 0) return props.allSlugs;
  return ["<endpoint>"];
});

const secret = computed(() => props.token || "<token>");

function command(slug: string): string {
  return (
    `claude mcp add --transport http api2mcp ${base}/mcp/${slug} \\\n` +
    `  --header "Authorization: Bearer ${secret.value}"`
  );
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <CodeBlock v-for="slug in slugs" :key="slug" :code="command(slug)" lang="shell" />
    <p class="text-xs text-neutral-500">
      Omit <code>--header</code> to authenticate through the browser with OAuth instead; the
      endpoint advertises its authorization server in the 401 it returns.
    </p>
  </div>
</template>
