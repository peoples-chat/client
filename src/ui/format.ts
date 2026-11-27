// Dates and times as the design writes them: 24-hour times and English words, like the rest
// of the interface.
const LOCALE = "en-GB";
const DAY = 86_400_000;

const time = new Intl.DateTimeFormat(LOCALE, { hour: "2-digit", minute: "2-digit" });
const weekday = new Intl.DateTimeFormat(LOCALE, { weekday: "long" });
const shortDate = new Intl.DateTimeFormat(LOCALE, { day: "numeric", month: "short" });
const longDate = new Intl.DateTimeFormat(LOCALE, { day: "numeric", month: "long", year: "numeric" });

function startOfDay(ms: number): number {
  const date = new Date(ms);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

function daysBefore(ms: number, now: number): number {
  return Math.round((startOfDay(now) - startOfDay(ms)) / DAY);
}

export function sameDay(a: number, b: number): boolean {
  return startOfDay(a) === startOfDay(b);
}

/** "14:08". */
export function formatTime(ms: number): string {
  return time.format(ms);
}

/** For the chat list: "14:08", "Yesterday", "Monday" or "12 Nov". */
export function formatListTime(ms: number, now = Date.now()): string {
  const days = daysBefore(ms, now);
  if (days <= 0) return formatTime(ms);
  if (days === 1) return "Yesterday";
  if (days < 7) return weekday.format(ms);
  return shortDate.format(ms);
}

/** For the day separators in a chat: "Today", "Yesterday", "Monday" or "12 November 2026". */
export function formatDay(ms: number, now = Date.now()): string {
  const days = daysBefore(ms, now);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return weekday.format(ms);
  return longDate.format(ms);
}

/** What a deleted message says in its place (#122). */
export const DELETED_TEXT = "This message was deleted.";

/** A message's text as the chat and the chat list show it: a deleted one says so (#122). */
export function shownText(message: { text: string; deleted?: boolean }): string {
  return message.deleted ? DELETED_TEXT : message.text;
}
