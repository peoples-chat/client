import { describe, expect, it } from "vitest";
import { radioBytes } from "./radio";

describe("radioBytes", () => {
  it("counts the UTF-8 bytes of the trimmed text", () => {
    expect(radioBytes("  Hello Bea ")).toBe(9);
    expect(radioBytes("Hej! Ses vi i morgen? ☔")).toBe(25);
    expect(radioBytes("æøå")).toBe(6);
    expect(radioBytes("   ")).toBe(0);
  });
});
