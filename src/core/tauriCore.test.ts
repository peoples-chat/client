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
    const account = { name: "Emil Dahl", about: "", publicKey: "ab", inviteLink: "nostr:nprofile1", shareLink: "https://peoples.chat/i#nprofile1" };
    const chat = shared.snapshot.chats[0];
    const message = chat.messages[1];
    const port = { path: "/dev/cu.usbmodem1101", description: "T1000-E" };
    const answers: Record<string, unknown> = {
      create_account: account,
      update_profile: account,
      set_profile_photo: account,
      remove_profile_photo: account,
      add_contact: chat,
      accept_invitation: chat,
      send_message: message,
      set_force_radio: { forced: true },
      cancel_message: null,
      radio_ports: [port],
      pair_radio: shared.snapshot.radio,
      forget_radio: { forced: false, canPair: true },
      export_account_key: "ncryptsec1fixture",
    };
    const { ipc, calls } = fakeIpc((command) => answers[command]);
    const core = createTauriCore(ipc);

    expect(await core.createAccount("Emil Dahl")).toEqual(account);
    expect(await core.updateProfile("Emil Dahl", "Aarhus")).toEqual(account);
    expect(await core.setProfilePhoto(new Uint8Array([0xff, 0xd8]))).toEqual(account);
    expect(await core.removeProfilePhoto()).toEqual(account);
    expect(await core.addContact("nostr:nprofile1abc")).toEqual(chat);
    expect(await core.acceptInvitation("inv-1")).toEqual(chat);
    expect(await core.sendMessage("chat-1", "Yes, around three.")).toEqual(message);
    expect(await core.setForceRadio(true)).toEqual({ forced: true });
    await core.cancelMessage("chat-1", "queued-1");
    expect(await core.radioPorts()).toEqual([port]);
    expect(await core.pairRadio(port.path)).toEqual(shared.snapshot.radio);
    expect(await core.forgetRadio()).toEqual({ forced: false, canPair: true });
    expect(await core.exportAccountKey({ kind: "protected", password: "a long password" })).toBe("ncryptsec1fixture");
    await core.exportAccountKey({ kind: "plain" });
    await core.deleteAccount();
    expect(calls.slice(1)).toEqual([
      'create_account {"name":"Emil Dahl"}',
      'update_profile {"name":"Emil Dahl","about":"Aarhus"}',
      'set_profile_photo {"0":255,"1":216}',
      "remove_profile_photo",
      'add_contact {"inviteLink":"nostr:nprofile1abc"}',
      'accept_invitation {"invitationId":"inv-1"}',
      'send_message {"chatId":"chat-1","text":"Yes, around three."}',
      'set_force_radio {"forced":true}',
      'cancel_message {"chatId":"chat-1","messageId":"queued-1"}',
      "radio_ports",
      'pair_radio {"port":"/dev/cu.usbmodem1101"}',
      "forget_radio",
      'export_account_key {"password":"a long password"}',
      'export_account_key {"password":null}',
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
});

describe("the JSON shared with the Rust core", () => {
  it("loads as a snapshot, and every event changes the state as it should", () => {
    let state: AppState = fromSnapshot(shared.snapshot);
    expect(state.account?.name).toBe("Emil Dahl");
    expect(state.chats[0].messages[1].delivery).toBe("confirmed");
    expect(state.chats[0].messages[2].delivery).toBe("timedOut");
    expect(state.invitations[0].from.name).toBe("Karen Winther");
    expect(state.account?.about).toBe("Aarhus. Usually reachable by radio.");
    expect(state.invites.map((i) => [i.name, i.joinedChatId])).toEqual([["Sofie", undefined]]);
    expect(state.relays.map((r) => r.operator)).toEqual(["peoplesChat", "independent"]);
    const paired = state.radio.paired;
    expect([paired?.name, paired?.connection, paired?.port, state.radio.forced, state.radio.canPair]).toEqual([
      "Emil's radio",
      "connected",
      "/dev/cu.usbmodem1101",
      false,
      true,
    ]);
    expect(paired?.preset).toEqual({
      frequencyKhz: 869618,
      bandwidthHz: 62500,
      spreadingFactor: 8,
      codingRate: 8,
      expected: true,
    });

    const checks: Record<CoreEvent["type"], (next: AppState) => void> = {
      chatAdded: (next) => expect(next.chats.map((c) => c.id)).toEqual(["chat-1", "chat-2"]),
      messageAdded: (next) => {
        const messages = next.chats[1].messages;
        expect(messages[messages.length - 1].delivery).toBe("waiting");
      },
      messageRemoved: (next) => expect(next.chats[1].messages.map((m) => m.cancellable)).toEqual([undefined]),
      deliveryChanged: (next) => expect(next.chats[1].messages[0].delivery).toBe("sent"),
      reachChanged: (next) =>
        expect(next.chats.map((c) => [c.reach, c.radioIssue])).toEqual([
          ["radio", undefined],
          ["unreachable", "notHeard"],
        ]),
      relaysChanged: (next) => expect(next.relays.map((r) => r.status)).toEqual(["connected", "unreachable"]),
      accountChanged: (next) =>
        expect([next.account?.name, next.account?.about, next.account?.photo?.slice(0, 23)]).toEqual([
          "Emil D.",
          "Aarhus, Denmark",
          "data:image/jpeg;base64,",
        ]),
      inviteChanged: (next) => expect(next.invites[0].joinedChatId).toBe("chat-1"),
      radioChanged: (next) => expect([next.radio.paired, next.radio.forced]).toEqual([undefined, true]),
      contactChanged: (next) =>
        expect([next.invitations[0].from.name, next.invitations[0].from.photo?.slice(0, 23)]).toEqual([
          "Karen W.",
          "data:image/jpeg;base64,",
        ]),
      invitationReceived: (next) => expect(next.invitations.map((i) => i.id)).toEqual(["inv-1", "inv-2"]),
      invitationRemoved: (next) => expect(next.invitations.map((i) => i.id)).toEqual(["inv-1"]),
      accountDeleted: (next) =>
        expect(next).toMatchObject({ account: null, chats: [], invitations: [], relays: [], radio: { forced: false } }),
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
