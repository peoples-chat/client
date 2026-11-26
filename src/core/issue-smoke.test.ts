import { describe, expect, it } from "vitest";
import { createMockCore } from "./mockCore";

describe("factory smoke test (#137)", () => {
  it("has at least one contact in the mock core's demo data", async () => {
    // No timers are started and nothing is stored: creating the account is what adds the demo data.
    const core = createMockCore({ storage: null });
    await core.createAccount("Emil Dahl");

    const { chats } = await core.load();
    const contacts = chats.map((chat) => chat.contact);

    expect(contacts.length).toBeGreaterThanOrEqual(1);
  });
});
