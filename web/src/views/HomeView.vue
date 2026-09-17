<script setup lang="ts">
// Landing view: a quick census of what's defined plus a link into each section. Not a "dashboard"
// with charts — this is an operator tool, and the honest summary of "what is there" is five counts
// and a health check, not a fabricated metric.
import { onMounted } from "vue";
import { useServicesStore } from "@/stores/services";
import { useAuthProvidersStore } from "@/stores/authProviders";
import { useApiCallsStore } from "@/stores/apiCalls";
import { useScriptsStore } from "@/stores/scripts";
import { useEndpointsStore } from "@/stores/endpoints";
import { useHealthStore } from "@/stores/health";
import StatusPill from "@/components/StatusPill.vue";

const services = useServicesStore();
const authProviders = useAuthProvidersStore();
const apiCalls = useApiCallsStore();
const scripts = useScriptsStore();
const endpoints = useEndpointsStore();
const health = useHealthStore();

onMounted(() => {
  services.fetchList();
  authProviders.fetchList();
  apiCalls.fetchList();
  scripts.fetchList();
  endpoints.fetchList();
  health.fetch();
});

const cards = [
  { to: "/services", label: "services", store: services },
  { to: "/auth-providers", label: "auth providers", store: authProviders },
  { to: "/api-calls", label: "api calls", store: apiCalls },
  { to: "/scripts", label: "scripts", store: scripts },
  { to: "/endpoints", label: "endpoints", store: endpoints },
];
</script>

<template>
  <div>
    <div class="mb-8">
      <h1 class="text-lg font-semibold tracking-wide text-ink">api2mcp admin</h1>
      <p class="mt-1 max-w-xl text-sm text-ink-dim">
        This is a rendering of what's curated, not a place where the API becomes less narrow. Every
        screen here shows a definition already in effect, plus the run log and resolved endpoint
        plans — the two things a YAML file on its own can't.
      </p>
    </div>

    <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
      <RouterLink
        v-for="card in cards"
        :key="card.to"
        :to="card.to"
        class="border border-border bg-surface p-4 hover:border-border-strong hover:bg-surface-raised"
      >
        <div class="text-2xl font-semibold text-ink">
          {{ card.store.loaded ? card.store.items.length : "—" }}
        </div>
        <div class="mt-1 text-[11px] tracking-[0.15em] text-ink-faint uppercase">{{ card.label }}</div>
      </RouterLink>
    </div>

    <div class="mt-8 flex items-center gap-3 border border-border bg-surface px-4 py-3">
      <StatusPill
        :label="health.data?.db_connected ? 'db connected' : 'db unreachable'"
        :tone="health.data?.db_connected ? 'ok' : 'error'"
      />
      <span v-if="health.data" class="text-xs text-ink-dim">
        v{{ health.data.version }} · migrations {{ health.data.migrations_applied }}/{{ health.data.migrations_total }}
        · {{ health.data.endpoint_count }} endpoint(s)
      </span>
      <RouterLink to="/health" class="ml-auto text-xs text-ink-faint hover:text-read">full health &rarr;</RouterLink>
    </div>
  </div>
</template>
