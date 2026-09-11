<script setup lang="ts">
// The persistent frame: nav on the left, a thin identity/status strip on top, routed content
// underneath. Mounted once in App.vue; every view renders inside its <main>.
import { onMounted } from "vue";
import { useSessionStore } from "@/stores/session";
import NavSidebar from "./NavSidebar.vue";

const session = useSessionStore();
onMounted(() => session.fetch());
</script>

<template>
  <div class="flex min-h-screen">
    <NavSidebar />
    <div class="flex min-w-0 flex-1 flex-col">
      <header class="flex items-center justify-end border-b border-border bg-surface px-6 py-2">
        <span v-if="session.me" class="text-xs text-ink-faint">
          signed in as <span class="text-ink-dim">{{ session.me.kind }}</span>
          <span v-if="session.me.is_admin" class="ml-1 text-read">· admin</span>
        </span>
      </header>
      <main class="min-w-0 flex-1 px-8 py-6">
        <slot />
      </main>
    </div>
  </div>
</template>
