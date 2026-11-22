import { describe, expect, it } from "vitest";
import type { Chat, Core, CoreEvent, CoreListener, Message, Relay, Snapshot } from "../core/types";
import { applyEvent, chatListItems, createStore, fromSnapshot, initialState } from "./store";

const message = (id: string, sentAt: number, extra: Partial<Message> = {}): Message => ({
  id,
  direction: "out",
  text: `Message ${id}`,
  sentAt,
  transport: "internet",
  delivery: "waiting",
  ...extra,
});

const chat = (id: string, messages: Message[] = []): Chat => ({
  id,
  contact: { publicKey: `key-${id}`, name: `Contact ${id}` },
  messages,
  reach: "internet",
});

const ready = (chats: Chat[] = []) =>
  fromSnapshot({ account: null, chats, invitations: [], relays: [], invites: [], radio: { forced: false } });

const relay = (url: string, status: Relay["status"]): Relay => ({ url, operator: "independent", status });

describe("applyEvent", () => {
  it("adds a message once, even when the event arrives twice", () => {
    const event: CoreEvent = { type: "messageAdded", chatId: "a", message: message("m1", 1) };
    const once = applyEvent(ready([chat("a")]), event);
    const twice = applyEvent(once, event);

    expect(once.chats[0].messages).toHaveLength(1);
    expect(twice).toBe(once);
  });

  it("changes the delivery state of one message", () => {
    const state = ready([chat("a", [message("m1", 1), message("m2", 2)])]);
    const next = applyEvent(state, { type: "deliveryChanged", chatId: "a", messageId: "m2", delivery: "sent" });

    expect(next.chats[0].messages.map((m) => m.delivery)).toEqual(["waiting", "sent"]);
    expect(next.chats[0].messages[0]).toBe(state.chats[0].messages[0]);
    expect(applyEvent(next, { type: "deliveryChanged", chatId: "a", messageId: "m2", delivery: "sent" })).toBe(next);
  });

  it("adds a message to the invitation with that id, before it is accepted", () => {
    const invitation = { id: "i1", from: { publicKey: "k", name: "Karen" }, receivedAt: 5, messages: [] };
    const state = { ...ready([chat("a")]), invitations: [invitation] };
    const incoming = message("m1", 6, { direction: "in", delivery: undefined });
    const once = applyEvent(state, { type: "messageAdded", chatId: "i1", message: incoming });

    expect(once.invitations[0].messages).toEqual([incoming]);
    expect(once.chats).toBe(state.chats);
    expect(applyEvent(once, { type: "messageAdded", chatId: "i1", message: incoming })).toBe(once);
  });

  it("changes how a chat's messages can travel", () => {
    const state = ready([chat("a"), chat("b")]);
    const next = applyEvent(state, { type: "reachChanged", chatId: "b", reach: "unreachable" });

    expect(next.chats.map((c) => c.reach)).toEqual(["internet", "unreachable"]);
    expect(next.chats[0]).toBe(state.chats[0]);
    expect(applyEvent(next, { type: "reachChanged", chatId: "b", reach: "unreachable" })).toBe(next);
  });

  it("replaces the relays, and changes nothing when they are the same", () => {
    const state = { ...ready(), relays: [relay("wss://a.example", "connected"), relay("wss://b.example", "connected")] };
    const relays = [relay("wss://a.example", "connected"), relay("wss://b.example", "unreachable")];
    const next = applyEvent(state, { type: "relaysChanged", relays });

    expect(next.relays).toEqual(relays);
    expect(applyEvent(next, { type: "relaysChanged", relays: structuredClone(relays) })).toBe(next);
  });

  it("takes the account's new profile, once", () => {
    const account = { name: "Emil", about: "", publicKey: "k", inviteLink: "peopleschat:x", shareLink: "https://peoples.chat/i#x" };
    const state = { ...ready(), account };
    const changed = { ...account, name: "Emil D.", about: "Aarhus" };
    const next = applyEvent(state, { type: "accountChanged", account: changed });

    expect(next.account).toEqual(changed);
    expect(applyEvent(next, { type: "accountChanged", account: { ...changed } })).toBe(next);
    // Without an account there is nothing to change.
    expect(applyEvent(ready(), { type: "accountChanged", account: changed }).account).toBeNull();
  });

  it("renames a contact in their chats and invitations", () => {
    const invitation = { id: "i1", from: { publicKey: "key-b", name: "Contact b" }, receivedAt: 5, messages: [] };
    const state = { ...ready([chat("a"), chat("b")]), invitations: [invitation] };
    const next = applyEvent(state, { type: "contactChanged", contact: { publicKey: "key-b", name: "Bea" } });

    expect(next.chats.map((c) => c.contact.name)).toEqual(["Contact a", "Bea"]);
    expect(next.chats[0]).toBe(state.chats[0]);
    expect(next.invitations[0].from.name).toBe("Bea");
    expect(applyEvent(next, { type: "contactChanged", contact: { publicKey: "key-b", name: "Bea" } })).toBe(next);
    expect(applyEvent(state, { type: "contactChanged", contact: { publicKey: "x", name: "Nobody" } })).toBe(state);
  });

  it("takes a contact's new photo, and drops it again", () => {
    const state = ready([chat("b")]);
    const withPhoto = { publicKey: "key-b", name: "Contact b", photo: "data:image/jpeg;base64,AA==" };
    const next = applyEvent(state, { type: "contactChanged", contact: withPhoto });

    expect(next.chats[0].contact.photo).toBe(withPhoto.photo);
    expect(applyEvent(next, { type: "contactChanged", contact: { ...withPhoto } })).toBe(next);
    const without = applyEvent(next, { type: "contactChanged", contact: { publicKey: "key-b", name: "Contact b" } });
    expect(without.chats[0].contact.photo).toBeUndefined();
  });

  it("removes a message, once", () => {
    const state = ready([chat("a", [message("m1", 1), message("m2", 2)])]);
    const event: CoreEvent = { type: "messageRemoved", chatId: "a", messageId: "m1" };
    const next = applyEvent(state, event);
    expect(next.chats[0].messages.map((m) => m.id)).toEqual(["m2"]);
    expect(applyEvent(next, event)).toBe(next);
    expect(applyEvent(state, { type: "messageRemoved", chatId: "x", messageId: "m1" })).toBe(state);
  });

  it("takes the mesh of a contact's radio", () => {
    const state = ready([chat("b")]);
    const withRadio = { publicKey: "key-b", name: "Contact b", meshes: ["meshCore" as const] };
    const next = applyEvent(state, { type: "contactChanged", contact: withRadio });

    expect(next.chats[0].contact.meshes).toEqual(["meshCore"]);
    expect(applyEvent(next, { type: "contactChanged", contact: { ...withRadio, meshes: ["meshCore"] } })).toBe(next);
  });

  it("adds a sent invitation, and marks it used once, when the person joins", () => {
    const invite = { id: "t1", name: "Sofie", link: "https://peoples.chat/i#x/t1", createdAt: 1, expiresAt: 2 };
    const added = applyEvent(ready(), { type: "inviteChanged", invite });
    expect(added.invites).toEqual([invite]);
    expect(applyEvent(added, { type: "inviteChanged", invite: { ...invite } })).toBe(added);

    const joined = applyEvent(added, { type: "inviteChanged", invite: { ...invite, joinedChatId: "c1" } });
    expect(joined.invites).toEqual([{ ...invite, joinedChatId: "c1" }]);
    expect(applyEvent(joined, { type: "accountDeleted" }).invites).toEqual([]);
  });

  it("ignores events for chats and messages it does not know", () => {
    const state = ready([chat("a")]);
    expect(applyEvent(state, { type: "messageAdded", chatId: "x", message: message("m1", 1) })).toBe(state);
    expect(applyEvent(state, { type: "deliveryChanged", chatId: "a", messageId: "x", delivery: "sent" })).toBe(state);
    expect(applyEvent(state, { type: "reachChanged", chatId: "x", reach: "unreachable" })).toBe(state);
    expect(applyEvent(state, { type: "invitationRemoved", invitationId: "x" })).toBe(state);
  });

  it("clears the account, chats, requests and relays when the account is deleted", () => {
    const state = {
      ...ready([chat("a")]),
      account: { name: "Emil", about: "", publicKey: "k", inviteLink: "peopleschat:x", shareLink: "https://peoples.chat/i#x" },
      invitations: [{ id: "i1", from: { publicKey: "k2", name: "Karen" }, receivedAt: 5, messages: [] }],
      relays: [relay("wss://a.example", "connected")],
    };
    expect(applyEvent(state, { type: "accountDeleted" })).toEqual({
      ...state,
      account: null,
      chats: [],
      invitations: [],
      relays: [],
    });
  });

  it("adds a chat and an invitation once, and removes the invitation", () => {
    const invitation = { id: "i1", from: { publicKey: "k", name: "Karen" }, receivedAt: 5, messages: [] };
    let state = applyEvent(ready(), { type: "chatAdded", chat: chat("a") });
    state = applyEvent(state, { type: "chatAdded", chat: chat("a") });
    state = applyEvent(state, { type: "invitationReceived", invitation });
    state = applyEvent(state, { type: "invitationReceived", invitation });

    expect(state.chats).toHaveLength(1);
    expect(state.invitations).toHaveLength(1);
    expect(applyEvent(state, { type: "invitationRemoved", invitationId: "i1" }).invitations).toEqual([]);
  });
});

describe("chatListItems", () => {
  it("lists requests and chats, newest first", () => {
    const state = {
      ...ready([chat("old", [message("m1", 10)]), chat("new", [message("m2", 30)])]),
      invitations: [{ id: "i1", from: { publicKey: "k", name: "Karen" }, receivedAt: 20, messages: [] }],
    };
    expect(chatListItems(state).map((item) => (item.kind === "chat" ? item.chat.id : item.invitation.id))).toEqual([
      "new",
      "i1",
      "old",
    ]);
  });
});

describe("createStore", () => {
  function fakeCore(snapshot: Snapshot) {
    let listener: CoreListener = () => {};
    let resolveLoad: (s: Snapshot) => void = () => {};
    const core = {
      load: () => new Promise<Snapshot>((resolve) => (resolveLoad = resolve)),
      subscribe: (l: CoreListener) => {
        listener = l;
        return () => {};
      },
    } as unknown as Core;
    return { core, emit: (event: CoreEvent) => listener(event), finishLoad: () => resolveLoad(snapshot) };
  }

  it("applies events that arrive while the snapshot loads", async () => {
    const { core, emit, finishLoad } = fakeCore({
      account: null,
      chats: [chat("a")],
      invitations: [],
      relays: [],
      invites: [],
      radio: { forced: false },
    });
    const store = createStore(core);
    let notified = 0;
    store.subscribe(() => notified++);

    const started = store.start();
    emit({ type: "messageAdded", chatId: "a", message: message("m1", 1) });
    expect(store.getState()).toBe(initialState);

    finishLoad();
    await started;
    expect(store.getState().status).toBe("ready");
    expect(store.getState().chats[0].messages).toHaveLength(1);

    emit({ type: "deliveryChanged", chatId: "a", messageId: "m1", delivery: "sent" });
    expect(store.getState().chats[0].messages[0].delivery).toBe("sent");
    expect(notified).toBe(2);
  });

  it("reports a core that fails to load", async () => {
    const core = { load: () => Promise.reject(new Error("no core")), subscribe: () => () => {} } as unknown as Core;
    const store = createStore(core);
    await store.start();
    expect(store.getState()).toMatchObject({ status: "failed", error: "no core" });
  });
});
