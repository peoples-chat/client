// Issue #129: the message request no longer suggests read receipts after accepting.
import { describe, expect, it } from "vitest";
import { requestNote } from "./MessageRequest";

describe("issue #129: message request note", () => {
  it("says the sender learns nothing from reading or accepting", () => {
    const note = requestNote("Bea Nygaard");
    expect(note).toBe("Bea is not told whether you read this or accept. They see only what you reply.");
    expect(note).not.toMatch(/until you accept/);
  });
});
