const encoder = new TextEncoder();

/** The size of `text` as radio counts it: UTF-8 bytes of the text the core sends, trimmed. */
export function radioBytes(text: string): number {
  return encoder.encode(text.trim()).length;
}
