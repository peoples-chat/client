import type { RadioPreset } from "./types";

const encoder = new TextEncoder();

/** The size of `text` as radio counts it: UTF-8 bytes of the text the core sends, trimmed. */
export function radioBytes(text: string): number {
  return encoder.encode(text.trim()).length;
}

/**
 * A radio's preset as Settings → Radio shows it (#24): "EU/UK (Narrow)" for the one the app
 * expects, otherwise the settings themselves, such as "869.525 MHz, 250 kHz, SF11, CR 4/5".
 */
export function presetText(preset: RadioPreset): string {
  if (preset.expected) return "EU/UK (Narrow)";
  const megahertz = preset.frequencyKhz / 1000;
  const kilohertz = preset.bandwidthHz / 1000;
  return `${megahertz} MHz, ${kilohertz} kHz, SF${preset.spreadingFactor}, CR 4/${preset.codingRate}`;
}
