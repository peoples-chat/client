// The interface's side of the boundary to the Rust core (#16). `docs/core-interface.md`
// describes the contract. The Rust side is `core/src/interface.rs`, and
// `core/tests/fixtures/interface.json` pins the JSON that both sides agree on.
//
// The web view sends commands and receives events. It holds no protocol logic and no key
// material: public keys and invite links are opaque strings here. `tauriCore.ts` reaches the
// real core through the app shell, and `mockCore.ts` implements the same interface with demo
// data.
//
// Not here yet: `pairRadio`, which arrives with the radio pairing work (#24).

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
  /**
   * The same as a web link, `https://peoples.chat/i#nprofile1…`, for sharing: tapped on a phone,
   * it opens the app (#72). Opaque to the interface.
   */
  shareLink: string;
}

export interface Contact {
  publicKey: PublicKey;
  name: string;
  /** Their profile photo, small and square, as a `data:image/jpeg` URL (#94). */
  photo?: string;
  /** The meshes their radios run, from their radio address records (#34). */
  meshes?: Mesh[];
}

/** A mesh network that radios run. */
export type Mesh = "meshCore";

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
  /** Why the chat isn't reachable by radio, when it is `unreachable` (#44). */
  radioIssue?: RadioIssue;
}

/**
 * Why a chat can't be reached by radio while no relay can reach it either (#44).
 * - `noRadio`: the user has no radio, or its link is down.
 * - `noContactRadio`: the contact hasn't shared a radio.
 * - `otherMesh`: the contact's radio runs another mesh.
 * - `otherPreset`: the contact's radio uses another region or preset.
 * - `notHeard`: the contact's radio hasn't been heard lately.
 */
export type RadioIssue = "noRadio" | "noContactRadio" | "otherMesh" | "otherPreset" | "notHeard";

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

/**
 * An invitation the user sent: a personal link that works once and for 7 days (#95). The person
 * who uses it lands in a chat with the user, without a request.
 */
export interface SentInvite {
  id: string;
  /** Whom it is for. Only the user sees it. */
  name: string;
  /** The link to share. Opaque to the interface. */
  link: string;
  createdAt: number;
  expiresAt: number;
  /** The chat that opened when the person joined, once they have. */
  joinedChatId?: string;
}

/** Everything the interface shows, as the core holds it now. */
/** The radio, as the core uses it (#40). */
export interface RadioStatus {
  /** The radio the core uses. Pairing one is #24; until then only the core's tests give it one. */
  paired?: PairedRadio;
  /**
   * Every message goes by radio: a switch for testing, until the core chooses the transport by
   * itself (#45).
   */
  forced: boolean;
}

export interface PairedRadio {
  /** The name the radio announces. */
  name: string;
  /** The radio's public key: its address on the mesh. Opaque to the interface. */
  publicKey: string;
  /** The link to the radio is open. */
  connected: boolean;
}

export interface Snapshot {
  account: Account | null;
  chats: Chat[];
  invitations: Invitation[];
  /** The account's relays: the project's first, then the others, each by address. Empty without an account. */
  relays: Relay[];
  /** The invitations the user sent, oldest first (#95). */
  invites: SentInvite[];
  radio: RadioStatus;
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
  | { type: "reachChanged"; chatId: string; reach: Reach; radioIssue?: RadioIssue }
  /** The account's relays changed, or their connections did. Carries all of them. */
  | { type: "relaysChanged"; relays: Relay[] }
  /** The user changed their profile (#93). */
  | { type: "accountChanged"; account: Account }
  /** A contact's profile changed, for example their name. Applies to their chats and invitations (#93). */
  | { type: "contactChanged"; contact: Contact }
  /** The user sent an invitation, or the person used it (#95). */
  | { type: "inviteChanged"; invite: SentInvite }
  /** The radio was paired, connected or disconnected, or the switch that forces it changed (#40). */
  | { type: "radioChanged"; radio: RadioStatus };

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
  /**
   * Makes a personal invitation for `name`, whom only the user sees (#95). Its link works once and
   * for 7 days, and opens the chat without a request.
   */
  createInvite(name: string): Promise<SentInvite>;
  /** Who an invite link is from, for someone who has no account yet (#95). */
  previewInvite(inviteLink: string): Promise<Contact>;
  /** Accepts an invitation. The chat it opens has the invitation's id. */
  acceptInvitation(invitationId: string): Promise<Chat>;
  /**
   * Sends text. The message is `waiting` until a relay has acknowledged it, then `sent`. With radio
   * forced, the radio carries it too, and it is `confirmed` once the other radio confirms it (#40).
   */
  sendMessage(chatId: string, text: string): Promise<Message>;
  /** Sends every message by radio, or stops doing so: a switch for testing (#40). */
  setForceRadio(forced: boolean): Promise<RadioStatus>;
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
