// Shared by JsonViewer and CodeBlock — both want an identical "copy, then show 'copied' briefly"
// affordance and neither should re-implement the timeout dance on its own.
import { ref } from "vue";

export function useClipboard(resetAfterMs = 1200) {
  const copied = ref(false);

  async function copy(text: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      copied.value = true;
      setTimeout(() => (copied.value = false), resetAfterMs);
    } catch {
      // Clipboard access can be denied by the browser; copying is a convenience, not
      // load-bearing, so failing silently is fine.
    }
  }

  return { copied, copy };
}
