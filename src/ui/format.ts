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

const UNITS: [seconds: number, name: string][] = [
  [7 * 86_400, "week"],
  [86_400, "day"],
  [3_600, "hour"],
  [60, "minute"],
  [1, "second"],
];

/**
 * A disappearing-message timer in words (#124): "1 week", "8 hours", "90 seconds". The largest
 * unit that divides it evenly, so that the number shown is exact.
 */
export function formatDuration(seconds: number): string {
  const [size, name] = UNITS.find(([size]) => seconds % size === 0) ?? UNITS[UNITS.length - 1];
  const count = Math.round(seconds / size);
  return `${count} ${name}${count === 1 ? "" : "s"}`;
}
