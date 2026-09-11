<script setup lang="ts">
// The one nav every view must be reachable from — the brief's own gate against orphan routes.
interface NavItem {
  to: string;
  label: string;
}

const groups: { heading: string; items: NavItem[] }[] = [
  {
    heading: "definitions",
    items: [
      { to: "/services", label: "services" },
      { to: "/auth-providers", label: "auth providers" },
      { to: "/api-calls", label: "api calls" },
      { to: "/scripts", label: "scripts" },
      { to: "/endpoints", label: "endpoints" },
    ],
  },
  {
    heading: "activity",
    items: [{ to: "/runs", label: "runs" }],
  },
  {
    heading: "system",
    items: [{ to: "/health", label: "health" }],
  },
];
</script>

<template>
  <nav class="flex h-full w-48 shrink-0 flex-col gap-6 border-r border-border bg-surface px-3 py-5">
    <RouterLink to="/" class="px-1 text-sm font-semibold tracking-[0.15em] text-ink uppercase">
      api2mcp
    </RouterLink>
    <div v-for="group in groups" :key="group.heading" class="flex flex-col gap-1">
      <div class="px-1 text-[10px] tracking-[0.2em] text-ink-faint uppercase">{{ group.heading }}</div>
      <RouterLink
        v-for="item in group.items"
        :key="item.to"
        :to="item.to"
        class="px-2 py-1.5 text-sm text-ink-dim hover:bg-surface-raised hover:text-ink"
        active-class="!bg-surface-raised !text-read"
      >
        {{ item.label }}
      </RouterLink>
    </div>
  </nav>
</template>
