import { describe, expect, it } from "vitest";
import { presetText, radioBytes } from "./radio";

describe("radioBytes", () => {
  it("counts the UTF-8 bytes of the trimmed text", () => {
    expect(radioBytes("  Hello Bea ")).toBe(9);
    expect(radioBytes("Hej! Ses vi i morgen? ☔")).toBe(25);
    expect(radioBytes("æøå")).toBe(6);
    expect(radioBytes("   ")).toBe(0);
  });
});

describe("presetText", () => {
  it("names the preset the app expects, and spells out any other (#24)", () => {
    expect(
      presetText({ frequencyKhz: 869_618, bandwidthHz: 62_500, spreadingFactor: 8, codingRate: 8, expected: true }),
    ).toBe("EU/UK (Narrow)");
    expect(
      presetText({ frequencyKhz: 869_525, bandwidthHz: 250_000, spreadingFactor: 11, codingRate: 5, expected: false }),
    ).toBe("869.525 MHz, 250 kHz, SF11, CR 4/5");
    // Close to it is still another preset: radios hear each other only with the same settings.
    expect(
      presetText({ frequencyKhz: 869_618, bandwidthHz: 62_500, spreadingFactor: 7, codingRate: 8, expected: false }),
    ).toBe("869.618 MHz, 62.5 kHz, SF7, CR 4/8");
  });
});
