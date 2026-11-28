import type { Message, Placeholder, Quote } from "../core/types";
import type { IconName } from "./Icon";

/** What a message the app can't show yet is called (#123). */
export const PLACEHOLDER_LABELS: Record<Placeholder, string> = {
  photo: "Photo",
  video: "Video",
  audio: "Audio",
  file: "File",
  sticker: "Sticker",
  poll: "Poll",
  chatEvent: "Chat update",
  unsupported: "Unsupported message",
};

export const PLACEHOLDER_ICONS: Record<Placeholder, IconName> = {
  photo: "image",
  video: "video",
  audio: "mic",
  file: "download",
  sticker: "image",
  poll: "checks",
  chatEvent: "info",
  unsupported: "help",
};

/** A quoted message that is deleted, or never reached this device. */
export const QUOTE_UNAVAILABLE = "Message not available";

/** One line for a message: its text, or what it is and its caption. For the chat list and quotes. */
export function messageSummary(message: Pick<Message, "text" | "placeholder">): string {
  const { text, placeholder } = message;
  if (!placeholder) return text;
  if (placeholder === "chatEvent") return text || PLACEHOLDER_LABELS.chatEvent;
  return text ? `${PLACEHOLDER_LABELS[placeholder]}: ${text}` : PLACEHOLDER_LABELS[placeholder];
}

/** The quoted message in one line, or that it isn't available. */
export function quoteSummary(quote: Quote): string {
  return quote.direction ? messageSummary(quote) : QUOTE_UNAVAILABLE;
}

/** "2 people reacted with ❤️", for screen readers. */
export function reactionLabel(emoji: string, count: number, mine: boolean): string {
  const who = mine ? (count === 1 ? "You" : `You and ${count - 1} more`) : count === 1 ? "1 person" : `${count} people`;
  return `${who} reacted with ${emoji}`;
}
