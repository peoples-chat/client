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
    expect(await core.load()).toEqual({
      account: null,
      chats: [],
      invitations: [],
      relays: [],
      invites: [],
      radio: { forced: false },
    });
  });

  it("creates an account with demo chats and a message request", async () => {
    const { core, events } = setup();
    const account = await core.createAccount("  Emil Dahl ");

    expect(account.name).toBe("Emil Dahl");
    expect(account.inviteLink).toMatch(/^peopleschat:demo-invite\//);
    expect(events.map((e) => e.type)).toEqual([
      "accountCreated",
      "relaysChanged",
      "chatAdded",
      "chatAdded",
      "invitationReceived",
    ]);

    const snapshot = await core.load();
    expect(snapshot.account).toEqual(account);
    // As in the design: one relay run by the project, and one that can't be reached.
    expect(snapshot.relays.map((r) => [r.operator, r.status])).toEqual([
      ["peoplesChat", "connected"],
      ["independent", "connected"],
      ["independent", "unreachable"],
    ]);
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

    // Radio carries 96 bytes at most; ☔ takes three. Longer text waits for the internet (#46).
    expect(await core.sendMessage(chat.id, "☔".repeat(33))).toMatchObject({ transport: "internet", cancellable: true });
    expect((await core.sendMessage(chat.id, "☔".repeat(32))).transport).toBe("radio");
  });

  it("sends everything by radio with the switch on, as short text the other radio confirms", async () => {
    const { core, events } = setup();
    await core.createAccount("Emil");
    const chat = (await core.load()).chats.find((c) => c.reach === "internet")!;
    events.length = 0;

    expect(await core.setForceRadio(true)).toEqual({ forced: true });
    expect(events).toEqual([{ type: "radioChanged", radio: { forced: true } }]);
    expect((await core.load()).radio).toEqual({ forced: true });

    const message = await core.sendMessage(chat.id, "By radio");
    expect(message).toMatchObject({ transport: "radio", delivery: "waiting" });
    vi.advanceTimersByTime(600);
    expect(events[2]).toEqual({ type: "deliveryChanged", chatId: chat.id, messageId: message.id, delivery: "confirmed" });
    await expect(core.sendMessage(chat.id, "x".repeat(97))).rejects.toThrow("too long for radio");

    await core.setForceRadio(false);
    expect((await core.sendMessage(chat.id, "x".repeat(97))).transport).toBe("internet");
  });

  it("lets a radio message time out when no radio confirms it", async () => {
    const { core, events } = setup({ radioConfirms: false });
    await core.createAccount("Emil");
    const chat = (await core.load()).chats.find((c) => c.reach === "radio")!;
    events.length = 0;

    const message = await core.sendMessage(chat.id, "Are you okay?");
    expect(message).toMatchObject({ transport: "radio", delivery: "waiting" });
    vi.advanceTimersByTime(11_999);
    expect(events).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(events[1]).toEqual({ type: "deliveryChanged", chatId: chat.id, messageId: message.id, delivery: "timedOut" });

    // Nobody answers a message that didn't arrive.
    vi.advanceTimersByTime(5000);
    expect(events).toHaveLength(2);
    const messages = (await core.load()).chats.find((c) => c.id === chat.id)!.messages;
    expect(messages[messages.length - 1]).toMatchObject({ id: message.id, delivery: "timedOut" });
  });

  it("keeps a message that has no path waiting, and lets the user cancel it", async () => {
    const { core, events } = setup();
    await core.createAccount("Emil");
    const radioChat = (await core.load()).chats.find((c) => c.reach === "radio")!;
    events.length = 0;

    // Too long for the radio that reaches the chat: it waits for the internet.
    const long = await core.sendMessage(radioChat.id, "x".repeat(97));
    expect(long).toMatchObject({ transport: "internet", delivery: "waiting", cancellable: true });
    vi.advanceTimersByTime(5000);
    expect(events).toEqual([{ type: "messageAdded", chatId: radioChat.id, message: long }]);

    await core.cancelMessage(radioChat.id, long.id);
    expect(events[1]).toEqual({ type: "messageRemoved", chatId: radioChat.id, messageId: long.id });
    expect((await core.load()).chats.find((c) => c.id === radioChat.id)!.messages.some((m) => m.id === long.id)).toBe(
      false,
    );
    await expect(core.cancelMessage(radioChat.id, long.id)).rejects.toThrow("no longer exists");
    const sent = await core.sendMessage(radioChat.id, "Short");
    await expect(core.cancelMessage(radioChat.id, sent.id)).rejects.toThrow("on its way already");
  });

  it("exports a demo copy of the key, protected by a long enough password or plain", async () => {
    const { core } = setup();
    await expect(core.exportAccountKey({ kind: "plain" })).rejects.toThrow("Create an account first.");
    await core.createAccount("Emil");
    await expect(core.exportAccountKey({ kind: "protected", password: "too short" })).rejects.toThrow(
      "at least 12 characters",
    );
    expect(await core.exportAccountKey({ kind: "protected", password: "long enough now" })).toMatch(/^ncryptsec1demo/);
    expect(await core.exportAccountKey({ kind: "plain" })).toMatch(/^nsec1demo/);
  });

  it("changes the profile's name and about, as the core checks them", async () => {
    const { core, events } = setup();
    await expect(core.updateProfile("Emil", "")).rejects.toThrow("Create an account first.");
    await core.createAccount("Emil");
    events.length = 0;

    const account = await core.updateProfile(" Emil D. ", " Aarhus ");
    expect([account.name, account.about]).toEqual(["Emil D.", "Aarhus"]);
    expect(events).toEqual([{ type: "accountChanged", account }]);
    expect((await core.load()).account).toEqual(account);
    // The same again is no change.
    await core.updateProfile("Emil D.", "Aarhus");
    expect(events).toHaveLength(1);

    await expect(core.updateProfile(" ", "")).rejects.toThrow("Enter a name.");
    await expect(core.updateProfile("n".repeat(65), "")).rejects.toThrow("up to 64 characters");
    await expect(core.updateProfile("Emil", "æ".repeat(281))).rejects.toThrow("up to 280 characters");
  });

  it("sets and removes the profile photo", async () => {
    const { core, events } = setup();
    await core.createAccount("Emil");
    events.length = 0;

    const account = await core.setProfilePhoto(new Uint8Array([0xff, 0xd8, 0xff]));
    expect(account.photo).toBe("data:image/jpeg;base64,/9j/");
    expect(events).toEqual([{ type: "accountChanged", account }]);
    await expect(core.setProfilePhoto(new Uint8Array())).rejects.toThrow("can't be used");

    const removed = await core.removeProfilePhoto();
    expect(removed.photo).toBeUndefined();
    expect((await core.load()).account?.photo).toBeUndefined();
    // Without a photo, removing it changes nothing.
    await core.removeProfilePhoto();
    expect(events).toHaveLength(2);
  });

  it("makes invitations with links of their own", async () => {
    const { core, events } = setup();
    await core.createAccount("Emil");
    events.length = 0;

    const invite = await core.createInvite(" Sofie ");
    expect(invite.name).toBe("Sofie");
    expect(invite.link).toMatch(/^https:\/\/peoples\.chat\/i#.+\/[0-9a-f]{20}$/);
    expect(events).toEqual([{ type: "inviteChanged", invite }]);
    expect((await core.load()).invites).toEqual([invite]);
    await expect(core.createInvite(" ")).rejects.toThrow("Enter a name.");
  });

  it("starts the chat with whoever a previewed invite link is from", async () => {
    const { core } = setup();
    const link = "https://peoples.chat/i#nprofile1demo/0123456789abcdef0123";
    const from = await core.previewInvite(link);
    expect(from.name).toBe("Emil Dahl");
    await expect(core.previewInvite("Emil Dahl")).rejects.toThrow("not an invite link");

    await core.createAccount("Sofie");
    const chat = await core.addContact(link);
    expect(chat.contact).toEqual(from);
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
    expect(await core.load()).toEqual({
      account: null,
      chats: [],
      invitations: [],
      relays: [],
      invites: [],
      radio: { forced: false },
    });

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
