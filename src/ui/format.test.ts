import { describe, expect, it } from "vitest";
import { formatDuration } from "./format";

describe("formatDuration", () => {
  it("issue 124: says a disappearing-message timer exactly, in the largest unit that fits", () => {
    expect(formatDuration(1)).toBe("1 second");
    expect(formatDuration(30)).toBe("30 seconds");
    expect(formatDuration(90)).toBe("90 seconds");
    expect(formatDuration(300)).toBe("5 minutes");
    expect(formatDuration(3600)).toBe("1 hour");
    expect(formatDuration(8 * 3600)).toBe("8 hours");
    expect(formatDuration(86_400)).toBe("1 day");
    expect(formatDuration(90 * 86_400)).toBe("90 days");
    expect(formatDuration(7 * 86_400)).toBe("1 week");
    expect(formatDuration(4 * 7 * 86_400)).toBe("4 weeks");
  });
});
