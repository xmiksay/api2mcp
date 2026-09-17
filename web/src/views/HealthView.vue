<script setup lang="ts">
import { onMounted } from "vue";
import { useHealthStore } from "@/stores/health";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import FieldRow from "@/components/FieldRow.vue";
import LoadingState from "@/components/LoadingState.vue";
import ErrorState from "@/components/ErrorState.vue";
import StatusPill from "@/components/StatusPill.vue";

const store = useHealthStore();
onMounted(() => store.fetch());
</script>

<template>
  <PageHeader title="Health" subtitle="Version, migration level, DB connectivity, endpoint count." />

  <LoadingState v-if="store.loading && !store.data" label="checking health" />
  <ErrorState v-else-if="store.error" :message="store.error" @retry="store.fetch" />
  <DetailSection v-else-if="store.data" title="status">
    <dl>
      <FieldRow label="version" mono>{{ store.data.version }}</FieldRow>
      <FieldRow label="commit" mono>{{ store.data.commit }}</FieldRow>
      <FieldRow label="database">
        <StatusPill
          :label="store.data.db_connected ? 'connected' : 'unreachable'"
          :tone="store.data.db_connected ? 'ok' : 'error'"
        />
      </FieldRow>
      <FieldRow label="migrations">
        {{ store.data.migrations_applied }} / {{ store.data.migrations_total }} applied
        <StatusPill
          v-if="store.data.migrations_applied < store.data.migrations_total"
          class="ml-2"
          label="pending migrations"
          tone="partial"
        />
      </FieldRow>
      <FieldRow label="endpoints">{{ store.data.endpoint_count }}</FieldRow>
    </dl>
  </DetailSection>
</template>
