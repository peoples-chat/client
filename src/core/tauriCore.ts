import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  CoreError,
  type Account,
  type Chat,
  type Contact,
  type Core,
  type CoreEvent,
  type CoreListener,
  type Message,
  type RadioStatus,
  type SentInvite,
  type Snapshot,
} from "./types";

/** The Tauri event that carries the core's events (`src-tauri/src/bridge.rs`). */
export const CORE_EVENT = "core-event";

/** The parts of Tauri's IPC that the core needs. Tests pass a fake. */
export interface Ipc {
  invoke<T>(command: string, args?: Record<string, unknown> | Uint8Array): Promise<T>;
  listen(event: string, handler: (event: { payload: CoreEvent }) => void): Promise<() => void>;
}

/**
 * The real Rust core in the app shell, reached through Tauri. Commands are Tauri commands, and
 * every change arrives as the Tauri event `core-event`. The contract: `docs/core-interface.md`.
 */
export function createTauriCore(ipc: Ipc = { invoke, listen }): Core {
  const listeners = new Set<CoreListener>();

  // Listen before the first snapshot is taken, so that no change falls between the two.
  const listening = ipc.listen(CORE_EVENT, ({ payload }) => {
    for (const listener of listeners) listener(payload);
  });
  listening.catch(() => undefined); // `load` reports the failure.

  async function call<T>(command: string, args?: Record<string, unknown> | Uint8Array): Promise<T> {
    try {
      return await ipc.invoke<T>(command, args);
    } catch (error) {
      throw toCoreError(error);
    }
  }

  return {
    async load() {
      await listening;
      return call<Snapshot>("core_snapshot");
    },

    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },

    createAccount: (name) => call<Account>("create_account", { name }),
    updateProfile: (name, about) => call<Account>("update_profile", { name, about }),
    // The photo goes as the request's raw body, not as JSON.
    setProfilePhoto: (photo) => call<Account>("set_profile_photo", photo),
    removeProfilePhoto: () => call<Account>("remove_profile_photo"),
    deleteAccount: () => call<void>("delete_account"),
    addContact: (inviteLink) => call<Chat>("add_contact", { inviteLink }),
    acceptInvitation: (invitationId) => call<Chat>("accept_invitation", { invitationId }),
    createInvite: (name) => call<SentInvite>("create_invite", { name }),
    previewInvite: (inviteLink) => call<Contact>("preview_invite", { inviteLink }),
    sendMessage: (chatId, text) => call<Message>("send_message", { chatId, text }),
    setForceRadio: (forced) => call<RadioStatus>("set_force_radio", { forced }),
    // Without a password the core exports the plain key.
    exportAccountKey: (copy) =>
      call<string>("export_account_key", { password: copy.kind === "protected" ? copy.password : null }),
  };
}

/** The core's errors carry a message to show as it is. Anything else is unexpected. */
function toCoreError(error: unknown): CoreError {
  if (typeof error === "object" && error !== null && "message" in error && typeof error.message === "string") {
    return new CoreError(error.message);
  }
  console.error("A core command failed:", error);
  return new CoreError("Something went wrong. Try again.");
}
