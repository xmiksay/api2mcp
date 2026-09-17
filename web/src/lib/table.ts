// Shared with DataTable.vue and every list view that configures one — kept out of the .vue file
// so it can be a plain `import type` everywhere without relying on type-only exports from an SFC.
export interface Column {
  key: string;
  label: string;
  /** Extra classes for both the header and body cell — e.g. `"text-right"` for a byte count. */
  class?: string;
}
