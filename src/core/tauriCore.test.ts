import { describe, expect, it, vi } from "vitest";
import fixture from "../../core/tests/fixtures/interface.json";
import { applyEvent, fromSnapshot, type AppState } from "../state/store";
import { CORE_EVENT, createTauriCore, type Ipc } from "./tauriCore";
import { CoreError, type CoreEvent, type Snapshot } from "./types";

/** The JSON the Rust core produces, shared with `core/tests/core_interface.rs`. */
const shared = fixture as unknown as { snapshot: Snapshot; events: CoreEvent[] };

/**
 * A fake of Tauri's IPC. It records calls, answers commands with `answer`, and registers the
 * event listener only when `finishListening` is called.
 */
function fakeIpc(answer: (command: string, args?: Record<string, unknown>) => unknown = () => undefined) {
  const calls: string[] = [];
  let handler: ((event: { payload: CoreEvent }) => void) | undefined;
  let finishListening = () => {};
  const registered = new Promise<void>((resolve) => (finishListening = resolve));

  const ipc: Ipc = {
    async invoke<T>(command: string, args?: Record<string, unknown>) {
      calls.push(args ? `${command} ${JSON.stringify(args)}` : command);
      return answer(command, args) as T;
    },
    async listen(event, eventHandler) {
      calls.push(`listen ${event}`);
      await registered;
      handler = eventHandler;
      return () => {
        handler = undefined;
      };
    },
  };
  const emit = (event: CoreEvent) => handler?.({ payload: event });
  return { ipc, calls, emit, finishListening: () => finishListening() };
}

describe("Tauri core", () => {
  it("asks for the snapshot only once it listens for events", async () => {
    const { ipc, calls, finishListening } = fakeIpc(() => shared.snapshot);
    const core = createTauriCore(ipc);

    const loading = core.load();
    await Promise.resolve();
    expect(calls).toEqual([`listen ${CORE_EVENT}`]);

    finishListening();
    expect(await loading).toEqual(shared.snapshot);
    expect(calls).toEqual([`listen ${CORE_EVENT}`, "core_snapshot"]);
  });

  it("passes core events to its subscribers", async () => {
    const { ipc, emit, finishListening } = fakeIpc(() => shared.snapshot);
    const core = createTauriCore(ipc);
    const received: CoreEvent[] = [];
    const unsubscribe = core.subscribe((event) => received.push(event));
    finishListening();
    await core.load();

    emit(shared.events[0]);
    unsubscribe();
    emit(shared.events[1]);
    expect(received).toEqual([shared.events[0]]);
  });

  it("sends commands with their arguments", async () => {
    const account = { name: "Emil Dahl", publicKey: "ab", inviteLink: "nostr:nprofile1" };
    const chat = shared.snapshot.chats[0];
    const answers: Record<string, unknown> = { create_account: account, add_contact: chat, accept_invitation: chat };
    const { ipc, calls } = fakeIpc((command) => answers[command]);
    const core = createTauriCore(ipc);

    expect(await core.createAccount("Emil Dahl")).toEqual(account);
    expect(await core.addContact("nostr:nprofile1abc")).toEqual(chat);
    expect(await core.acceptInvitation("inv-1")).toEqual(chat);
    await core.deleteAccount();
    expect(calls.slice(1)).toEqual([
      'create_account {"name":"Emil Dahl"}',
      'add_contact {"inviteLink":"nostr:nprofile1abc"}',
      'accept_invitation {"invitationId":"inv-1"}',
      "delete_account",
    ]);
  });

  it("shows the core's error messages as they are", async () => {
    const { ipc } = fakeIpc(() => {
      throw { message: "Enter a name." };
    });
    await expect(createTauriCore(ipc).createAccount(" ")).rejects.toEqual(new CoreError("Enter a name."));
  });

  it("turns unexpected failures into a general error", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const { ipc } = fakeIpc(() => {
      throw "the IPC broke";
    });
    const failing = createTauriCore(ipc).deleteAccount();
    await expect(failing).rejects.toBeInstanceOf(CoreError);
    await expect(failing).rejects.toThrow("Something went wrong. Try again.");
    expect(logged).toHaveBeenCalledOnce();
    logged.mockRestore();
  });

  it("refuses what the core cannot do yet, without asking it", async () => {
    const { ipc, calls } = fakeIpc();
    const core = createTauriCore(ipc);

    await expect(core.sendMessage("chat-1", "Hi")).rejects.toThrow("Sending messages is not available yet.");
    expect(calls).toEqual([`listen ${CORE_EVENT}`]);
  });
});

describe("the JSON shared with the Rust core", () => {
  it("loads as a snapshot, and every event changes the state as it should", () => {
    let state: AppState = fromSnapshot(shared.snapshot);
    expect(state.account?.name).toBe("Emil Dahl");
    expect(state.chats[0].messages[1].delivery).toBe("confirmed");
    expect(state.invitations[0].from.name).toBe("Karen Winther");

    const checks: Record<CoreEvent["type"], (next: AppState) => void> = {
      chatAdded: (next) => expect(next.chats.map((c) => c.id)).toEqual(["chat-1", "chat-2"]),
      messageAdded: (next) => expect(next.chats[1].messages[0].delivery).toBe("waiting"),
      deliveryChanged: (next) => expect(next.chats[1].messages[0].delivery).toBe("sent"),
      invitationReceived: (next) => expect(next.invitations.map((i) => i.id)).toEqual(["inv-1", "inv-2"]),
      invitationRemoved: (next) => expect(next.invitations.map((i) => i.id)).toEqual(["inv-1"]),
      accountDeleted: (next) => expect(next).toMatchObject({ account: null, chats: [], invitations: [] }),
      accountCreated: (next) => expect(next.account?.publicKey).toBe(shared.snapshot.account?.publicKey),
    };

    for (const event of shared.events) {
      const next = applyEvent(state, event);
      expect(next, event.type).not.toBe(state);
      checks[event.type](next);
      state = next;
    }
    expect(new Set(shared.events.map((e) => e.type))).toEqual(new Set(Object.keys(checks)));
  });
});
