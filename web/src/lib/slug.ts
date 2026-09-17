// Mirrors `model::Slug`'s `FromStr` (src/model/slug.rs): lowercase ascii, digits, `_`/`-`, 1-64
// bytes. Client-side only as a fast "this will be rejected" hint before the round trip — the
// server's own parse is still the actual gate.
const SLUG_PATTERN = /^[a-z0-9_-]+$/;

export function slugError(value: string): string | null {
  if (value.length === 0) return "required";
  if (value.length > 64) return `too long (${value.length} bytes, max 64)`;
  if (!SLUG_PATTERN.test(value)) return "only a-z, 0-9, _ and - are allowed";
  return null;
}
