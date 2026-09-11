<script setup lang="ts">
// One param, fully editable. The fixed/caller toggle is the loudest thing on the card — on
// purpose: `Param::is_model_visible` (a fixed param never reaches the generated schema, never
// overridable by a caller) is the exact distinction the brief calls "the product," so it has to
// be obvious while editing, not just legible afterward in ParamTable's read-only badge.
import { computed } from "vue";
import type { PackParam, ParamType } from "@/api";
import { checkboxClass, fieldClass, monoFieldClass, smallIconButtonClass } from "@/lib/formStyle";
import { HEADER_PARAM_ALLOWLIST } from "@/lib/headerAllowlist";
import { LOCATION_LABELS, bodyPointer, buildLocation, locationKind, type LocationKind } from "@/lib/paramLocation";
import ParamValueInput from "./ParamValueInput.vue";

const props = defineProps<{ param: PackParam; allowedLocations: LocationKind[] }>();
const emit = defineEmits<{ "update:param": [PackParam]; remove: [] }>();

const PARAM_TYPES: ParamType[] = ["string", "integer", "number", "boolean", "string_array"];
const ENUM_ELIGIBLE: ParamType[] = ["string", "integer", "number"];

function patch(fields: Partial<PackParam>): void {
  emit("update:param", { ...props.param, ...fields });
}

const isFixed = computed(() => props.param.fixed !== undefined);

function setFixed(fixed: boolean): void {
  if (fixed) {
    // A fixed param can never also be required (`CHECK(fixed_value IS NULL OR required = false)`
    // on the server side) — force it off here so the two can't drift apart in the UI either.
    patch({ fixed: defaultValueFor(props.param.type), required: false });
  } else {
    patch({ fixed: undefined });
  }
}

function defaultValueFor(ty: ParamType): unknown {
  switch (ty) {
    case "boolean":
      return false;
    case "integer":
    case "number":
      return 0;
    case "string_array":
      return [];
    default:
      return "";
  }
}

function setType(ty: ParamType): void {
  patch({ type: ty, default: undefined, fixed: isFixed.value ? defaultValueFor(ty) : undefined, enum_values: undefined });
}

function setLocationKind(kind: LocationKind): void {
  const name = kind === "header" ? (HEADER_PARAM_ALLOWLIST[0] as string) : props.param.name;
  patch({ location: buildLocation(kind, bodyPointer(props.param.location)), name });
}

function setBodyPointer(pointer: string): void {
  patch({ location: buildLocation("body", pointer) });
}

function enumText(values: unknown[] | undefined): string {
  return (values ?? []).map((v) => String(v)).join(", ");
}

function setEnumValues(raw: string): void {
  const parts = raw
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  if (parts.length === 0) {
    patch({ enum_values: undefined });
    return;
  }
  const values = parts.map((s) => (props.param.type === "string" ? s : Number(s)));
  patch({ enum_values: values });
}
</script>

<template>
  <div class="border border-border-strong bg-surface-raised/40 p-3">
    <div class="mb-3 flex flex-wrap items-center gap-2">
      <select
        v-if="locationKind(param.location) === 'header'"
        :value="param.name"
        :class="monoFieldClass + ' max-w-56'"
        @change="patch({ name: ($event.target as HTMLSelectElement).value })"
      >
        <option v-for="h in HEADER_PARAM_ALLOWLIST" :key="h" :value="h">{{ h }}</option>
      </select>
      <input
        v-else
        :value="param.name"
        placeholder="param name"
        :class="monoFieldClass + ' max-w-56'"
        @input="patch({ name: ($event.target as HTMLInputElement).value })"
      />

      <select
        :value="locationKind(param.location)"
        :class="fieldClass + ' max-w-40'"
        @change="setLocationKind(($event.target as HTMLSelectElement).value as LocationKind)"
      >
        <option v-for="kind in allowedLocations" :key="kind" :value="kind">{{ LOCATION_LABELS[kind] }}</option>
      </select>

      <select :value="param.type" :class="fieldClass + ' max-w-36'" @change="setType(($event.target as HTMLSelectElement).value as ParamType)">
        <option v-for="ty in PARAM_TYPES" :key="ty" :value="ty">{{ ty }}</option>
      </select>

      <!-- The fixed/caller segmented toggle — colors match ParamTable's read-only badge exactly. -->
      <div class="ml-auto flex overflow-hidden border" :class="isFixed ? 'border-write/40' : 'border-read/40'">
        <button
          type="button"
          class="px-2 py-1 text-[10px] tracking-wide uppercase"
          :class="!isFixed ? 'bg-read/20 text-read' : 'text-ink-faint hover:text-ink-dim'"
          @click="setFixed(false)"
        >
          caller-supplied
        </button>
        <button
          type="button"
          class="px-2 py-1 text-[10px] tracking-wide uppercase"
          :class="isFixed ? 'bg-write/20 text-write' : 'text-ink-faint hover:text-ink-dim'"
          @click="setFixed(true)"
        >
          definer-fixed
        </button>
      </div>
      <button type="button" :class="smallIconButtonClass" title="remove param" @click="emit('remove')">&times;</button>
    </div>

    <input
      v-if="locationKind(param.location) === 'body'"
      :value="bodyPointer(param.location)"
      placeholder="/json/pointer/into/body"
      :class="monoFieldClass + ' mb-3'"
      @input="setBodyPointer(($event.target as HTMLInputElement).value)"
    />

    <div v-if="isFixed" class="border border-write/30 bg-write/5 p-2">
      <p class="mb-1.5 text-[10px] tracking-[0.1em] text-write uppercase">
        fixed value — always sent, never visible to or overridable by the model
      </p>
      <ParamValueInput
        :type="param.type"
        :model-value="param.fixed"
        @update:model-value="(v) => patch({ fixed: v })"
      />
    </div>

    <div v-else class="flex flex-col gap-2">
      <label class="flex items-center gap-2 text-xs text-ink-dim">
        <input
          type="checkbox"
          :class="checkboxClass"
          :checked="param.required"
          @change="patch({ required: ($event.target as HTMLInputElement).checked })"
        />
        required
      </label>

      <div class="flex items-center gap-2">
        <label class="flex shrink-0 items-center gap-2 text-xs text-ink-dim">
          <input
            type="checkbox"
            :class="checkboxClass"
            :checked="param.default !== undefined"
            @change="patch({ default: ($event.target as HTMLInputElement).checked ? defaultValueFor(param.type) : undefined })"
          />
          default
        </label>
        <ParamValueInput
          v-if="param.default !== undefined"
          :type="param.type"
          :model-value="param.default"
          @update:model-value="(v) => patch({ default: v })"
        />
      </div>

      <div v-if="ENUM_ELIGIBLE.includes(param.type)" class="flex items-center gap-2">
        <span class="w-24 shrink-0 text-xs text-ink-dim">enum values</span>
        <input
          :value="enumText(param.enum_values)"
          placeholder="comma-separated — leave blank for no restriction"
          :class="fieldClass"
          @input="setEnumValues(($event.target as HTMLInputElement).value)"
        />
      </div>
    </div>

    <input
      :value="param.description ?? ''"
      placeholder="description (shown to the model when caller-supplied)"
      :class="fieldClass + ' mt-2'"
      @input="patch({ description: ($event.target as HTMLInputElement).value || undefined })"
    />
  </div>
</template>
