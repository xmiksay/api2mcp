// Shared class strings for the write forms — kept in one place so every form's inputs/buttons
// read as the same instrument panel the read views already establish, rather than each form
// re-deriving its own idea of "what a text input looks like here."
export const fieldClass =
  "w-full border border-border-strong bg-surface px-2 py-1.5 text-sm text-ink placeholder:text-ink-faint focus:border-read focus:outline-none disabled:cursor-not-allowed disabled:opacity-50";

export const monoFieldClass = `${fieldClass} font-mono`;

export const textareaClass = `${fieldClass} resize-y`;

export const checkboxClass = "h-4 w-4 border border-border-strong bg-surface accent-read";

/** A neutral, low-emphasis action — "cancel", "add row", "remove". */
export const ghostButtonClass =
  "border border-border-strong px-3 py-1.5 text-xs tracking-wide text-ink-dim uppercase hover:border-read hover:text-read disabled:cursor-not-allowed disabled:opacity-40";

/** The read-tinted affordance already used for "view plan" — reused here for every "Save". */
export const primaryButtonClass =
  "border border-read/50 px-3 py-1.5 text-xs tracking-wide text-read uppercase hover:bg-read/10 disabled:cursor-not-allowed disabled:opacity-40";

/** Reused for "new X" header actions and other write-tinted affordances. */
export const writeButtonClass =
  "border border-write/50 px-3 py-1.5 text-xs tracking-wide text-write uppercase hover:bg-write/10 disabled:cursor-not-allowed disabled:opacity-40";

export const dangerButtonClass =
  "border border-error/50 px-3 py-1.5 text-xs tracking-wide text-error uppercase hover:bg-error/10 disabled:cursor-not-allowed disabled:opacity-40";

export const smallIconButtonClass =
  "border border-border-strong px-1.5 py-0.5 text-xs text-ink-faint hover:border-error hover:text-error";
