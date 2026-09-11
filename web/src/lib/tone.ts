// Maps domain values to the fixed status palette declared in `style.css`'s `@theme` block, so
// "what color means what" lives in exactly one place instead of being re-decided per component.
import type { Access, RunStatus } from "@/api";

export type Tone = "read" | "write" | "ok" | "partial" | "error" | "denied" | "budget" | "timeout" | "neutral";

export function accessTone(access: Access): Tone {
  return access === "write" ? "write" : "read";
}

export function runStatusTone(status: RunStatus): Tone {
  switch (status) {
    case "ok":
      return "ok";
    case "partial":
      return "partial";
    case "error":
      return "error";
    case "denied":
      return "denied";
    case "budget_exceeded":
      return "budget";
    case "timeout":
      return "timeout";
  }
}

export function boolTone(value: boolean, whenTrue: Tone = "ok", whenFalse: Tone = "error"): Tone {
  return value ? whenTrue : whenFalse;
}

const CLASSES: Record<Tone, string> = {
  read: "text-read border-read/40 bg-read/10",
  write: "text-write border-write/40 bg-write/10",
  ok: "text-ok border-ok/40 bg-ok/10",
  partial: "text-partial border-partial/40 bg-partial/10",
  error: "text-error border-error/40 bg-error/10",
  denied: "text-denied border-denied/40 bg-denied/10",
  budget: "text-budget border-budget/40 bg-budget/10",
  timeout: "text-timeout border-timeout/40 bg-timeout/10",
  neutral: "text-ink-dim border-border-strong bg-white/5",
};

export function toneClasses(tone: Tone): string {
  return CLASSES[tone];
}
