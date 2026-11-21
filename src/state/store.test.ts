import { describe, expect, it } from "vitest";
import type { Chat, Core, CoreEvent, CoreListener, Message, Snapshot } from "../core/types";
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
});

const ready = (chats: Chat[] = []) => fromSnapshot({ account: null, chats, invitations: [] });

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

  it("ignores events for chats and messages it does not know", () => {
    const state = ready([chat("a")]);
    expect(applyEvent(state, { type: "messageAdded", chatId: "x", message: message("m1", 1) })).toBe(state);
    expect(applyEvent(state, { type: "deliveryChanged", chatId: "a", messageId: "x", delivery: "sent" })).toBe(state);
    expect(applyEvent(state, { type: "invitationRemoved", invitationId: "x" })).toBe(state);
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
    const { core, emit, finishLoad } = fakeCore({ account: null, chats: [chat("a")], invitations: [] });
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
