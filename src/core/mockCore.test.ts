import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createMockCore, type MockCoreOptions } from "./mockCore";
import { CoreError, type CoreEvent } from "./types";

const NOW = Date.UTC(2026, 10, 21, 14, 0);

function setup(options: MockCoreOptions = {}) {
  const core = createMockCore({ storage: null, now: () => NOW, ...options });
  const events: CoreEvent[] = [];
  core.subscribe((event) => events.push(event));
  return { core, events };
}

function memoryStorage() {
  const items = new Map<string, string>();
  return {
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => void items.set(key, value),
  };
}

describe("mock core", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts without an account", async () => {
    const { core } = setup();
    expect(await core.load()).toEqual({ account: null, chats: [], invitations: [] });
  });

  it("creates an account with demo chats and a message request", async () => {
    const { core, events } = setup();
    const account = await core.createAccount("  Emil Dahl ");

    expect(account.name).toBe("Emil Dahl");
    expect(account.inviteLink).toMatch(/^peopleschat:demo-invite\//);
    expect(events.map((e) => e.type)).toEqual(["accountCreated", "chatAdded", "chatAdded", "invitationReceived"]);

    const snapshot = await core.load();
    expect(snapshot.account).toEqual(account);
    expect(snapshot.chats.map((c) => c.reach)).toEqual(["internet", "radio"]);
    expect(snapshot.invitations[0].messages[0].text).toMatch(/^Hi Emil,/);
  });

  it("refuses an empty name and a second account", async () => {
    const { core } = setup();
    await expect(core.createAccount("   ")).rejects.toBeInstanceOf(CoreError);
    await core.createAccount("Emil");
    await expect(core.createAccount("Emil")).rejects.toThrow("already has an account");
  });

  it("acknowledges a sent message, then lets the contact reply", async () => {
    const { core, events } = setup();
    await core.createAccount("Emil");
    const [chat] = (await core.load()).chats;
    events.length = 0;

    const message = await core.sendMessage(chat.id, "  Hello  ");
    expect(message).toMatchObject({ direction: "out", text: "Hello", delivery: "waiting", transport: "internet" });
    expect(events).toEqual([{ type: "messageAdded", chatId: chat.id, message }]);

    vi.advanceTimersByTime(600);
    expect(events[1]).toEqual({ type: "deliveryChanged", chatId: chat.id, messageId: message.id, delivery: "sent" });

    vi.advanceTimersByTime(1500);
    expect(events[2]).toMatchObject({ type: "messageAdded", chatId: chat.id, message: { direction: "in" } });
    expect((await core.load()).chats[0].messages).toHaveLength(4);
  });

  it("sends by radio where the chat is reached by radio, and the other radio confirms", async () => {
    const { core, events } = setup();
    await core.createAccount("Emil");
    const chat = (await core.load()).chats.find((c) => c.reach === "radio")!;
    events.length = 0;

    const message = await core.sendMessage(chat.id, "Are you okay?");
    expect(message).toMatchObject({ transport: "radio", delivery: "waiting" });
    vi.advanceTimersByTime(600);
    expect(events[1]).toEqual({ type: "deliveryChanged", chatId: chat.id, messageId: message.id, delivery: "confirmed" });
    vi.advanceTimersByTime(1500);
    expect(events[2]).toMatchObject({ type: "messageAdded", message: { direction: "in", transport: "radio" } });

    // Radio carries 96 bytes at most; ☔ takes three.
    await expect(core.sendMessage(chat.id, "☔".repeat(33))).rejects.toThrow("too long for radio");
    await core.sendMessage(chat.id, "☔".repeat(32));
  });

  it("refuses an empty message and an unknown chat", async () => {
    const { core } = setup();
    await core.createAccount("Emil");
    const [chat] = (await core.load()).chats;
    await expect(core.sendMessage(chat.id, "  ")).rejects.toBeInstanceOf(CoreError);
    await expect(core.sendMessage("missing", "Hello")).rejects.toBeInstanceOf(CoreError);
  });

  it("starts a chat from an invite link, and reuses it for the same link", async () => {
    const { core, events } = setup();
    await core.createAccount("Emil");
    events.length = 0;

    const chat = await core.addContact(" peopleschat:demo-invite/abc123 ");
    expect(chat.contact.name).toBe("Jonas Kjær");
    expect(chat.messages).toEqual([]);
    expect(events).toEqual([{ type: "chatAdded", chat }]);

    expect(await core.addContact("peopleschat:demo-invite/abc123")).toEqual(chat);
    expect(events).toHaveLength(1);
  });

  it("explains why a name or its own link cannot be added", async () => {
    const { core } = setup();
    const account = await core.createAccount("Emil");
    await expect(core.addContact("Astrid Holm")).rejects.toThrow("Paste an invite link");
    await expect(core.addContact(account.inviteLink)).rejects.toThrow("your own invite link");
  });

  it("turns an accepted invitation into a chat with its messages", async () => {
    const { core, events } = setup();
    await core.createAccount("Emil");
    const [invitation] = (await core.load()).invitations;
    events.length = 0;

    const chat = await core.acceptInvitation(invitation.id);
    expect(chat.contact).toEqual(invitation.from);
    expect(chat.messages).toEqual(invitation.messages);
    expect(events).toEqual([
      { type: "invitationRemoved", invitationId: invitation.id },
      { type: "chatAdded", chat },
    ]);
    expect((await core.load()).invitations).toEqual([]);
    await expect(core.acceptInvitation(invitation.id)).rejects.toBeInstanceOf(CoreError);
  });

  it("deletes the account with its chats and requests, and allows a new one", async () => {
    const { core, events } = setup();
    await core.createAccount("Emil");
    const [chat] = (await core.load()).chats;
    await core.sendMessage(chat.id, "Hello");
    events.length = 0;

    await core.deleteAccount();
    expect(events).toEqual([{ type: "accountDeleted" }]);
    expect(await core.load()).toEqual({ account: null, chats: [], invitations: [] });

    // The pending acknowledgement and reply belonged to the deleted account.
    vi.advanceTimersByTime(5000);
    expect(events).toHaveLength(1);

    await core.createAccount("Karen");
    expect((await core.load()).account?.name).toBe("Karen");
  });

  it("refuses to delete an account that does not exist", async () => {
    const { core } = setup();
    await expect(core.deleteAccount()).rejects.toBeInstanceOf(CoreError);
  });

  it("forgets a deleted account across reloads", async () => {
    const storage = memoryStorage();
    const first = createMockCore({ storage, now: () => NOW });
    await first.createAccount("Emil");
    await first.deleteAccount();
    expect((await createMockCore({ storage }).load()).account).toBeNull();
  });

  it("keeps its state between reloads and settles waiting messages", async () => {
    const storage = memoryStorage();
    const first = createMockCore({ storage, now: () => NOW });
    await first.createAccount("Emil");
    const [chat] = (await first.load()).chats;
    await first.sendMessage(chat.id, "Before the reload");

    const second = createMockCore({ storage, now: () => NOW });
    const snapshot = await second.load();
    const messages = snapshot.chats[0].messages;
    expect(snapshot.account?.name).toBe("Emil");
    expect(messages[messages.length - 1]).toMatchObject({ text: "Before the reload", delivery: "sent" });
  });

  it("starts fresh when the saved state is unreadable", async () => {
    const storage = memoryStorage();
    storage.setItem("peoples-chat.mock-core", "{not json");
    const core = createMockCore({ storage });
    expect((await core.load()).account).toBeNull();
  });
});
