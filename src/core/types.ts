// The interface's side of the boundary to the Rust core (#16). `docs/core-interface.md`
// describes the contract. The Rust side is `core/src/interface.rs`, and
// `core/tests/fixtures/interface.json` pins the JSON that both sides agree on.
//
// The web view sends commands and receives events. It holds no protocol logic and no key
// material: public keys and invite links are opaque strings here. `tauriCore.ts` reaches the
// real core through the app shell, and `mockCore.ts` implements the same interface with demo
// data.
//
// Not here yet: `pairRadio` and the radio state events. They arrive with the radio pairing work
// (#24) and the transport work (#48).

/** Public key of an account, formatted by the core. Opaque to the interface. */
export type PublicKey = string;

export interface Account {
  name: string;
  publicKey: PublicKey;
  /**
   * Link that other people open or scan to start a chat. The user's QR code encodes it. From the
   * real core it is a `nostr:nprofile1…` URI; to the interface it is opaque.
   */
  inviteLink: string;
}

export interface Contact {
  publicKey: PublicKey;
  name: string;
}

/** How a message travelled. */
export type Transport = "internet" | "radio";

/**
 * The longest text one radio message carries, in UTF-8 bytes: what fits in one packet
 * (`docs/radio-format-v0.md`, #38). Longer text waits for the internet.
 */
export const RADIO_TEXT_LIMIT = 96;

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

/**
 * How a chat's messages can travel now (#43).
 * - `internet`: at least one of the chat's relays is connected.
 * - `radio`: no relay is reachable, but the contact's radio has been heard (#44).
 * - `unreachable`: neither. Messages wait.
 */
export type Reach = "internet" | "radio" | "unreachable";

/** A direct chat with one contact. */
export interface Chat {
  id: string;
  contact: Contact;
  /** Oldest first. */
  messages: Message[];
  reach: Reach;
}

/** An invitation to a direct chat from someone who is not a contact yet. */
export interface Invitation {
  /** The id of the chat that accepting the invitation opens. */
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
 * sent messages, so the interface has one path for adding a message to a chat. Its `chatId` can
 * also be an invitation's id: messages that arrive before the user accepts.
 */
export type CoreEvent =
  | { type: "accountCreated"; account: Account }
  | { type: "accountDeleted" }
  | { type: "chatAdded"; chat: Chat }
  | { type: "invitationReceived"; invitation: Invitation }
  | { type: "invitationRemoved"; invitationId: string }
  | { type: "messageAdded"; chatId: string; message: Message }
  | { type: "deliveryChanged"; chatId: string; messageId: string; delivery: DeliveryState }
  | { type: "reachChanged"; chatId: string; reach: Reach };

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
  /**
   * Starts a direct chat from a scanned QR code or a pasted invite link, and sends the other
   * person an invitation. With an existing contact it returns their chat.
   */
  addContact(inviteLink: string): Promise<Chat>;
  /** Accepts an invitation. The chat it opens has the invitation's id. */
  acceptInvitation(invitationId: string): Promise<Chat>;
  /** Sends text. The message is `waiting` until a relay has acknowledged it, then `sent`. */
  sendMessage(chatId: string, text: string): Promise<Message>;
}

/** A command failed for a reason the user can act on. The message is shown as it is. */
export class CoreError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CoreError";
  }
}
