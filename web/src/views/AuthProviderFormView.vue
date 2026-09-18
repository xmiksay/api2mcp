<script setup lang="ts">
// New/edit form for an auth provider (I5). A provider names exactly one credential source: an
// environment variable the server reads at request time, or a value stored on its own row — the
// per-owner case, where a process-wide env var cannot give each owner their own token. The stored
// value is write-only in both directions: it is never returned by a read, so an edit that leaves
// the field blank sends nothing and the server keeps whatever is already stored.
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { ApiError } from "@/api";
import type { AuthProviderWrite, PackAuthKind, PackAuthProvider } from "@/api";
import { useAuthProvidersStore } from "@/stores/authProviders";
import { useServicesStore } from "@/stores/services";
import PageHeader from "@/components/PageHeader.vue";
import DetailSection from "@/components/DetailSection.vue";
import LoadingState from "@/components/LoadingState.vue";
import FormField from "@/components/form/FormField.vue";
import ValidationErrors from "@/components/form/ValidationErrors.vue";
import StringListEditor from "@/components/form/StringListEditor.vue";
import { fieldClass, ghostButtonClass, primaryButtonClass } from "@/lib/formStyle";
import { slugError } from "@/lib/slug";

const props = defineProps<{ mode: "create" | "edit"; slug?: string }>();
const router = useRouter();
const store = useAuthProvidersStore();
const services = useServicesStore();

const KINDS: PackAuthKind[] = ["static_header", "oauth2_client_credentials"];

type CredentialSource = "env" | "stored";

function blank(): PackAuthProvider {
  return {
    service: services.items[0]?.slug ?? "",
    kind: "static_header",
    credential_env_key: "",
    header_name: "Authorization",
    value_template: "Bearer ",
    scopes: [],
    token_url: undefined,
    bound_origin: "",
  };
}

const slugInput = ref(props.slug ?? "");
const form = ref<PackAuthProvider>(blank());
const source = ref<CredentialSource>("env");
const credentialValue = ref("");
const hasStoredCredential = ref(false);
const submitting = ref(false);
const errors = ref<string[]>([]);
const loading = ref(props.mode === "edit");

onMounted(async () => {
  await services.fetchList();
  if (props.mode === "create") form.value = blank();
  if (props.mode === "edit" && props.slug) {
    const existing = await store.fetchOne(props.slug);
    if (existing) {
      form.value = { ...existing };
      // An absent env key is what marks a stored-source provider — the value itself never
      // arrives here, only whether one is set.
      source.value = existing.credential_env_key ? "env" : "stored";
      hasStoredCredential.value = existing.has_stored_credential;
    }
    loading.value = false;
  }
});

const slugProblem = computed(() => (props.mode === "create" ? slugError(slugInput.value) : null));
const boundService = computed(() => services.items.find((s) => s.slug === form.value.service) ?? null);

async function submit(): Promise<void> {
  if (slugProblem.value) return;
  submitting.value = true;
  errors.value = [];
  // An empty string is not "no token url" on the wire (`token_url: Option<String>`) — it's a
  // string that then fails `url::Url::parse`, so blank out to `undefined` before sending.
  const payload: AuthProviderWrite = { ...form.value, token_url: form.value.token_url || undefined };
  if (source.value === "env") {
    // The server decides the source from this field alone, so a stored value must not linger.
    payload.credential_value = undefined;
  } else {
    payload.credential_env_key = undefined;
    // Blank means "leave whatever is stored alone", which is also the right thing on create:
    // the provider is created with no value and the owner sets one when they have it.
    payload.credential_value = credentialValue.value || undefined;
  }
  try {
    if (props.mode === "create") {
      await store.create(slugInput.value, payload);
      router.push(`/auth-providers/${slugInput.value}`);
    } else if (props.slug) {
      await store.update(props.slug, payload);
      router.push(`/auth-providers/${props.slug}`);
    }
  } catch (e) {
    errors.value = e instanceof ApiError ? e.messages : ["request failed"];
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <LoadingState v-if="loading" label="loading auth provider" />
  <div v-else>
    <PageHeader
      :title="mode === 'create' ? 'New Auth Provider' : `Edit ${props.slug}`"
      subtitle="auth provider — human-only writes (I5)"
      :back="{ to: mode === 'create' ? '/auth-providers' : `/auth-providers/${props.slug}`, label: mode === 'create' ? 'auth providers' : props.slug! }"
    />

    <ValidationErrors :errors="errors" />

    <form class="flex flex-col gap-6" @submit.prevent="submit">
      <DetailSection title="identity">
        <div class="flex flex-col gap-4">
          <FormField v-if="mode === 'create'" label="slug" required :errors="slugProblem ? [slugProblem] : []">
            <input v-model="slugInput" class="max-w-64 font-mono" :class="fieldClass" placeholder="my-provider" />
          </FormField>
          <p v-else class="font-mono text-sm text-ink">{{ props.slug }}</p>

          <FormField label="service" required :help="mode === 'edit' ? 'cannot be changed after creation — delete and recreate instead' : undefined">
            <select v-model="form.service" :class="fieldClass + ' max-w-64'" :disabled="mode === 'edit'">
              <option v-for="s in services.items" :key="s.slug" :value="s.slug">{{ s.slug }}</option>
            </select>
          </FormField>

          <FormField label="kind" required>
            <select v-model="form.kind" :class="fieldClass + ' max-w-64'">
              <option v-for="k in KINDS" :key="k" :value="k">{{ k }}</option>
            </select>
          </FormField>
        </div>
      </DetailSection>

      <DetailSection title="credential wiring">
        <div class="flex flex-col gap-4">
          <FormField
            label="credential source"
            required
            help="An environment variable suits one credential shared by everyone. Store the value here instead when each owner needs their own token for the same upstream — a process-wide variable cannot express that."
          >
            <select v-model="source" :class="fieldClass + ' max-w-64'">
              <option value="env">environment variable</option>
              <option value="stored">stored value</option>
            </select>
          </FormField>
          <FormField
            v-if="source === 'env'"
            label="credential env key"
            required
            help="The NAME of an environment variable on the server — e.g. STRIPE_API_TOKEN. Never paste a token or secret value into this field: the actual credential is read from that variable at request time and is never stored in this database or shown on any screen."
          >
            <input v-model="form.credential_env_key" :class="fieldClass + ' font-mono'" placeholder="SERVICE_API_TOKEN" />
          </FormField>
          <FormField
            v-else
            label="credential value"
            :help="hasStoredCredential
              ? 'A value is already stored. Leave blank to keep it; type a new one to replace it. It is stored in plaintext and is never shown again after saving.'
              : 'Stored in plaintext in the database and never shown again after saving — anyone with access to the database or a backup of it can read it. Leave blank to save the provider now and set the value later.'"
          >
            <input
              v-model="credentialValue"
              type="password"
              autocomplete="new-password"
              :class="fieldClass + ' font-mono'"
              :placeholder="hasStoredCredential ? 'unchanged' : 'glpat-…'"
            />
          </FormField>
          <FormField label="header name" required>
            <input v-model="form.header_name" :class="fieldClass + ' font-mono'" placeholder="Authorization" />
          </FormField>
          <FormField
            label="header value prefix"
            required
            help="Literal text prepended to the credential value when building the header — e.g. &quot;Bearer &quot; (with the trailing space). The credential is appended directly after this text; this field never contains the credential itself."
          >
            <input v-model="form.value_template" :class="fieldClass + ' font-mono'" placeholder="Bearer " />
          </FormField>
          <FormField label="bound origin" required help="Must be one of the service's allowlisted origins (I5) — the credential is only ever attached to a request going to this exact origin.">
            <select v-if="boundService && boundService.origin_allowlist.length > 0" v-model="form.bound_origin" :class="fieldClass + ' max-w-64 font-mono'">
              <option v-for="o in boundService.origin_allowlist" :key="o" :value="o">{{ o }}</option>
            </select>
            <input v-else v-model="form.bound_origin" :class="fieldClass + ' font-mono'" placeholder="https://api.example.com" />
          </FormField>
          <FormField v-if="form.kind === 'oauth2_client_credentials'" label="token url">
            <input v-model="form.token_url" :class="fieldClass + ' font-mono'" placeholder="https://api.example.com/oauth/token" />
          </FormField>
          <FormField label="scopes">
            <StringListEditor v-model="form.scopes" placeholder="read:things" />
          </FormField>
        </div>
      </DetailSection>

      <div class="flex items-center gap-3">
        <button type="submit" :disabled="submitting" :class="primaryButtonClass">
          {{ submitting ? "saving…" : "save" }}
        </button>
        <RouterLink :to="mode === 'create' ? '/auth-providers' : `/auth-providers/${props.slug}`" :class="ghostButtonClass">cancel</RouterLink>
      </div>
    </form>
  </div>
</template>
