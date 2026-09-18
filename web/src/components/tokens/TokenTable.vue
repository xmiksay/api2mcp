<script setup lang="ts">
// The list half of the tokens screen. Revoke lives here (not in the parent view) since it's a
// per-row concern with its own in-flight/error state — same shape as *DetailView's single delete,
// just tracked per id instead of one flag.
import { ref } from "vue";
import type { TokenView } from "@/api";
import { ApiError } from "@/api";
import { useTokensStore } from "@/stores/tokens";
import DataTable from "@/components/DataTable.vue";
import StatusPill from "@/components/StatusPill.vue";
import TagChips from "@/components/TagChips.vue";
import ConfirmButton from "@/components/form/ConfirmButton.vue";
import type { Column } from "@/lib/table";
import { tokenState, tokenStateTone } from "@/lib/tone";
import { formatDateTime } from "@/lib/format";

defineProps<{ tokens: TokenView[] }>();
const store = useTokensStore();

const columns: Column[] = [
  { key: "token_prefix", label: "prefix" },
  { key: "label", label: "label" },
  { key: "endpoints", label: "endpoints" },
  { key: "reach", label: "reach" },
  { key: "created_at", label: "created" },
  { key: "last_used_at", label: "last used" },
  { key: "expires_at", label: "expires" },
  { key: "state", label: "state" },
  { key: "actions", label: "", class: "text-right" },
];

const revokingId = ref<string | null>(null);
const revokeError = ref<string | null>(null);

async function revoke(id: string): Promise<void> {
  revokingId.value = id;
  revokeError.value = null;
  try {
    await store.revoke(id);
  } catch (e) {
    revokeError.value = e instanceof ApiError ? e.message : "revoke failed";
  } finally {
    revokingId.value = null;
  }
}

function orNever(iso: string | null): string {
  return iso ? formatDateTime(iso) : "never";
}
</script>

<template>
  <div>
    <p v-if="revokeError" class="mb-3 border border-error/40 bg-error/10 px-3 py-2 text-xs text-error">
      {{ revokeError }}
    </p>
    <DataTable :columns="columns" :rows="tokens" :row-key="(t) => t.id">
      <template #cell-token_prefix="{ row }">
        <span class="font-mono text-ink">{{ row.token_prefix }}&hellip;</span>
      </template>
      <!-- Whether the token can define things, as opposed to only calling them — a different
           kind of power from the endpoint grant beside it, so it gets its own column. -->
      <template #cell-reach="{ row }">
        <span v-if="row.control_plane" class="text-xs text-write">control plane</span>
        <span v-else class="text-xs text-ink-faint">tools only</span>
      </template>

      <template #cell-endpoints="{ row }">
        <span v-if="row.endpoints.length === 0" class="text-xs text-ink-faint italic">all endpoints</span>
        <TagChips v-else :tags="row.endpoints" />
      </template>
      <template #cell-created_at="{ row }">{{ formatDateTime(row.created_at) }}</template>
      <template #cell-last_used_at="{ row }">{{ orNever(row.last_used_at) }}</template>
      <template #cell-expires_at="{ row }">{{ orNever(row.expires_at) }}</template>
      <template #cell-state="{ row }">
        <StatusPill
          :label="tokenState(row.revoked_at, row.expires_at)"
          :tone="tokenStateTone(tokenState(row.revoked_at, row.expires_at))"
        />
      </template>
      <template #cell-actions="{ row }">
        <ConfirmButton
          v-if="!row.revoked_at"
          label="revoke"
          title="An MCP client using this token stops working immediately — this cannot be undone."
          :pending="revokingId === row.id"
          @confirm="revoke(row.id)"
        />
        <span v-else class="text-xs text-ink-faint">&mdash;</span>
      </template>
    </DataTable>
  </div>
</template>
