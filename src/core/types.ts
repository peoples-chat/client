// The interface's side of the boundary to the Rust core (#16).
//
// The web view sends commands and receives events. It holds no protocol logic and no key
// material: public keys and invite links are opaque strings here. Until the real core is
// wired through Tauri, `mockCore.ts` implements this interface with demo data.
//
// Not here yet: `pairRadio` and the transport and radio state events. They arrive with the
// radio pairing work (#24) and the transport work (#48).

/** Public key of an account, formatted by the core. Opaque to the interface. */
export type PublicKey = string;

export interface Account {
  name: string;
  publicKey: PublicKey;
  /** Link that other people open or scan to start a chat. The user's QR code encodes it. */
  inviteLink: string;
}

export interface Contact {
  publicKey: PublicKey;
  name: string;
}

/** How a message travelled. */
export type Transport = "internet" | "radio";

/**
 * Delivery of an outgoing message.
 * - `waiting`: not sent yet, for example because there is no path.
 * - `sent`: a relay acknowledged it.
 * - `confirmed`: the other radio confirmed it. Radio only.
 */
export type DeliveryState = "waiting" | "sent" | "confirmed";

export interface Message {
  /** Stable identifier from the core. Duplicates are dropped by it after an outage. */
  id: string;
  direction: "in" | "out";
  text: string;
  /** Milliseconds since the Unix epoch. */
  sentAt: number;
  transport: Transport;
  /** Set on outgoing messages only. */
  delivery?: DeliveryState;
}

/** A direct chat with one contact. */
export interface Chat {
  id: string;
  contact: Contact;
  /** Oldest first. */
  messages: Message[];
}

/** An invitation to a direct chat from someone who is not a contact yet. */
export interface Invitation {
  id: string;
  from: Contact;
  receivedAt: number;
  /** Messages that came with the invitation. Shown before the user accepts. */
  messages: Message[];
}

/** Everything the interface shows, as the core holds it now. */
export interface Snapshot {
  account: Account | null;
  chats: Chat[];
  invitations: Invitation[];
}

/**
 * Changes pushed by the core. `messageAdded` covers both received messages and the user's own
 * sent messages, so the interface has one path for adding a message to a chat.
 */
export type CoreEvent =
  | { type: "accountCreated"; account: Account }
  | { type: "accountDeleted" }
  | { type: "chatAdded"; chat: Chat }
  | { type: "invitationReceived"; invitation: Invitation }
  | { type: "invitationRemoved"; invitationId: string }
  | { type: "messageAdded"; chatId: string; message: Message }
  | { type: "deliveryChanged"; chatId: string; messageId: string; delivery: DeliveryState };

export type CoreListener = (event: CoreEvent) => void;

/**
 * Commands from the interface. A command resolves once the core has taken it, and the
 * resulting state changes arrive as events, also for the command's own changes.
 */
export interface Core {
  /** The current state to start from. Every later change arrives as an event. */
  load(): Promise<Snapshot>;
  subscribe(listener: CoreListener): () => void;

  /** First launch: creates the account key and publishes the profile name. */
  createAccount(name: string): Promise<Account>;
  /** Removes the profile, leaves every chat and deletes the account's data from this device. */
  deleteAccount(): Promise<void>;
  /** Starts a direct chat from a scanned QR code or a pasted invite link. */
  addContact(inviteLink: string): Promise<Chat>;
  acceptInvitation(invitationId: string): Promise<Chat>;
  sendMessage(chatId: string, text: string): Promise<Message>;
}

/** A command failed for a reason the user can act on. The message is shown as it is. */
export class CoreError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CoreError";
  }
}
