import { describe, expect, it } from "vitest";
import { undeliveredText } from "./undelivered";

describe("issue 126: a chat whose invitation hasn't reached the contact", () => {
  it("says so, and that the app keeps trying", () => {
    expect(undeliveredText({ since: 1 }, "Bea")).toBe(
      "Bea hasn't received your invitation yet, so they can't see this chat. The app keeps trying.",
    );
  });

  it("gives the reason when the core has one", () => {
    expect(undeliveredText({ since: 1, reason: "tooLarge" }, "Bea")).toBe(
      "Bea hasn't received your invitation yet, so they can't see this chat. A relay refused it as too large. " +
        "The app tries again when relays change.",
    );
  });
});
