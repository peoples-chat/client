export type Tint = "green" | "neutral" | "amber";

const TINTS: Tint[] = ["amber", "green", "neutral"];

/** Up to two initials: "Astrid Holm" becomes "AH", "Mikkel Østergaard" becomes "MØ". */
export function initials(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "";
  const first = Array.from(words[0])[0];
  const last = words.length > 1 ? Array.from(words[words.length - 1])[0] : "";
  return (first + last).toUpperCase();
}

export function firstName(name: string): string {
  return name.trim().split(/\s+/)[0] ?? name;
}

/** A stable avatar tint for a name (djb2 hash), so a person keeps their colour. */
export function tintFor(name: string): Tint {
  let hash = 5381;
  for (const char of name) hash = ((hash << 5) + hash + (char.codePointAt(0) ?? 0)) >>> 0;
  return TINTS[hash % TINTS.length];
}
