// #125: a chat that stops being a two-person chat stays, and says what it is now.
import { describe, expect, it } from "vitest";
import type { Chat } from "../core/types";
import { applyEvent, canSend, chatEndedText, fromSnapshot } from "./store";

const bea = { publicKey: "b".repeat(64), name: "Bea Holm" };

const chat = (extra: Partial<Chat> = {}): Chat => ({
  id: "chat-1",
  contact: bea,
  messages: [{ id: "m1", direction: "in", text: "Hello", sentAt: 1, transport: "internet" }],
  reach: "internet",
  ...extra,
});

const ready = (chats: Chat[]) =>
  fromSnapshot({ account: null, chats, invitations: [], relays: [], invites: [], radio: { forced: false } });

describe("issue #125: a chat that stops being direct", () => {
  it("replaces the chat with its id when it changes, and keeps it in the list", () => {
    const other = chat({ id: "chat-2" });
    const before = ready([chat(), other]);
    const left = chat({ state: "contactLeft" });
    const after = applyEvent(before, { type: "chatChanged", chat: left });

    expect(after.chats.map((c) => c.id)).toEqual(["chat-1", "chat-2"]);
    expect(after.chats[0]).toEqual(left);
    expect(after.chats[1]).toBe(other);
    // The same chat again changes nothing.
    expect(applyEvent(after, { type: "chatChanged", chat: structuredClone(left) })).toBe(after);
  });

  it("brings each message's sender once the chat is a group", () => {
    const group = chat({
      state: "group",
      messages: [
        { id: "m1", direction: "in", text: "Hello", sentAt: 1, transport: "internet", sender: bea },
        {
          id: "m2",
          direction: "in",
          text: "Hi, I'm new",
          sentAt: 2,
          transport: "internet",
          sender: { publicKey: "c".repeat(64), name: "Carl Ek" },
        },
      ],
    });
    const after = applyEvent(ready([chat()]), { type: "chatChanged", chat: group });
    expect(after.chats[0].messages.map((m) => m.sender?.name)).toEqual(["Bea Holm", "Carl Ek"]);
  });

  it("says the contact left, and stops sending there", () => {
    expect(canSend(chat())).toBe(true);
    expect(chatEndedText(chat(), "Bea")).toBeNull();

    const left = chat({ state: "contactLeft" });
    expect(canSend(left)).toBe(false);
    expect(chatEndedText(left, "Bea")).toBe("Bea left this chat. Messages can't be sent here any more.");

    const removed = chat({ state: "removed" });
    expect(canSend(removed)).toBe(false);
    expect(chatEndedText(removed, "Bea")).toMatch(/^You are no longer in this chat/);
  });

  it("says a chat with a name or another member is no longer direct, and still sends there", () => {
    const group = chat({ state: "group" });
    expect(canSend(group)).toBe(true);
    expect(chatEndedText(group, "Bea")).toBe("This is no longer a direct chat: it has a name or more people now.");
  });
});
