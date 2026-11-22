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
  /** A few words about the user, from the profile. Empty when there are none (#93). */
  about: string;
  /** The profile photo, small and square, as a `data:image/jpeg` URL (#94). */
  photo?: string;
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
  /** Their profile photo, small and square, as a `data:image/jpeg` URL (#94). */
  photo?: string;
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

/**
 * Who runs a relay (#88).
 * - `peoplesChat`: the project.
 * - `independent`: anyone else.
 */
export type RelayOperator = "peoplesChat" | "independent";

/**
 * Whether the account's connection to a relay is open.
 * - `connected`: it is.
 * - `connecting`: not connected, but not for long enough to count as gone: opening, or a short
 *   drop.
 * - `unreachable`: closed for long enough to count as gone (#43).
 */
export type RelayStatus = "connected" | "connecting" | "unreachable";

/** One of the relays the account's messages travel through (#88). */
export interface Relay {
  /** The relay's address, such as `wss://nos.lol`. */
  url: string;
  operator: RelayOperator;
  status: RelayStatus;
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
  /** The account's relays: the project's first, then the others, each by address. Empty without an account. */
  relays: Relay[];
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
  | { type: "reachChanged"; chatId: string; reach: Reach }
  /** The account's relays changed, or their connections did. Carries all of them. */
  | { type: "relaysChanged"; relays: Relay[] }
  /** The user changed their profile (#93). */
  | { type: "accountChanged"; account: Account }
  /** A contact's profile changed, for example their name. Applies to their chats and invitations (#93). */
  | { type: "contactChanged"; contact: Contact };

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
  /**
   * Changes the profile's name and about, and publishes them (#93). The name is required; about
   * can be empty. Needs a relay: offline, it fails and nothing changes.
   */
  updateProfile(name: string, about: string): Promise<Account>;
  /**
   * Sets the profile photo (#94). The core turns it upright, cuts it square, shrinks it and saves
   * it again as JPEG, which drops its metadata, then uploads it and publishes its address.
   */
  setProfilePhoto(photo: Uint8Array): Promise<Account>;
  /** Removes the profile photo (#94). */
  removeProfilePhoto(): Promise<Account>;
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
  /**
   * Exports the account key once the device's owner has unlocked it (#86). A protected copy is
   * a NIP-49 `ncryptsec1…` that other Nostr apps open with its password; a plain one is the
   * `nsec1…` itself. Keep the result only while it is shown.
   */
  exportAccountKey(copy: KeyCopy): Promise<string>;
}

/** What kind of copy of the account key to export. */
export type KeyCopy = { kind: "protected"; password: string } | { kind: "plain" };

/** The longest profile name, in characters (`MAX_NAME_CHARS` in the core). */
export const MAX_NAME = 64;

/** The longest about text, in characters (`MAX_ABOUT_CHARS` in the core). */
export const MAX_ABOUT = 280;

/** The shortest password for a protected copy of the account key. */
export const MIN_EXPORT_PASSWORD = 12;

/** A command failed for a reason the user can act on. The message is shown as it is. */
export class CoreError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CoreError";
  }
}
