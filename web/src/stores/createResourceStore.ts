// Shared shape behind every slug-addressed definition store (services, auth_providers, api_calls,
// scripts, endpoints): a cached list plus per-slug detail fetch/upsert. Extracted once five stores
// turned out to need the identical loading/error/cache dance — see `stores/services.ts` etc. for
// the thin instantiations.
import { computed, ref, shallowRef } from "vue";
import { defineStore } from "pinia";
import { ApiError } from "@/api";

export interface SlugResourceApi<T> {
  list(): Promise<T[]>;
  get(slug: string): Promise<T>;
}

/** The write half — every definition store's API module already shapes up as this. */
export interface WritableResourceApi<T, D> extends SlugResourceApi<T> {
  create(slug: string, def: D): Promise<T>;
  update(slug: string, def: D): Promise<T>;
  remove(slug: string): Promise<void>;
}

function messageOf(e: unknown): string {
  return e instanceof ApiError ? e.message : "request failed";
}

/**
 * Builds a Pinia store definition for one slug-addressed resource. Called once per resource at
 * module scope (each call site passes its own store id), which is the standard way to generate
 * several distinct stores from one factory.
 *
 * Create/update/remove intentionally do not catch `ApiError` themselves — a write form needs the
 * *shape* of a validation failure (`ApiError.messages`, plural) to show every problem at once,
 * which a store-level string would collapse. The caller (the form) owns its own submit/error UI
 * state; this store only owns the resulting cache mutation once a write actually succeeds.
 */
export function createResourceStore<T extends { slug: string }, D>(
  id: string,
  resource: WritableResourceApi<T, D>,
) {
  return defineStore(id, () => {
    // `shallowRef`, not `ref`: with a generic element type, `ref<T[]>` collides with Vue's
    // `UnwrapRefSimple<T>` machinery (a known limitation, not a real reactivity need here — every
    // update below replaces the array wholesale anyway).
    const items = shallowRef<T[]>([]);
    const loaded = ref(false);
    const loading = ref(false);
    const error = ref<string | null>(null);

    const detailLoading = ref(false);
    const detailError = ref<string | null>(null);

    async function fetchList(force = false): Promise<void> {
      if (loaded.value && !force) return;
      loading.value = true;
      error.value = null;
      try {
        items.value = await resource.list();
        loaded.value = true;
      } catch (e) {
        error.value = messageOf(e);
      } finally {
        loading.value = false;
      }
    }

    /** Always hits the network — a direct navigation to a detail URL can't rely on the list cache. */
    async function fetchOne(slug: string): Promise<T | null> {
      detailLoading.value = true;
      detailError.value = null;
      try {
        const item = await resource.get(slug);
        const idx = items.value.findIndex((i) => i.slug === slug);
        items.value =
          idx >= 0
            ? items.value.map((existing, i) => (i === idx ? item : existing))
            : [...items.value, item];
        return item;
      } catch (e) {
        detailError.value = messageOf(e);
        return null;
      } finally {
        detailLoading.value = false;
      }
    }

    function bySlug(slug: string) {
      return computed(() => items.value.find((i) => i.slug === slug) ?? null);
    }

    /** Throws `ApiError` on failure — see this factory's own doc for why that's deliberate. */
    async function create(slug: string, def: D): Promise<T> {
      const item = await resource.create(slug, def);
      items.value = [...items.value, item];
      return item;
    }

    async function update(slug: string, def: D): Promise<T> {
      const item = await resource.update(slug, def);
      const idx = items.value.findIndex((i) => i.slug === slug);
      items.value = idx >= 0 ? items.value.map((existing, i) => (i === idx ? item : existing)) : [...items.value, item];
      return item;
    }

    async function remove(slug: string): Promise<void> {
      await resource.remove(slug);
      items.value = items.value.filter((i) => i.slug !== slug);
    }

    return {
      items,
      loaded,
      loading,
      error,
      detailLoading,
      detailError,
      fetchList,
      fetchOne,
      bySlug,
      create,
      update,
      remove,
    };
  });
}
