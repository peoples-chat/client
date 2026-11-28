import { radioBytes } from "./radio";
import {
  CoreError,
  MAX_ABOUT,
  MAX_NAME,
  MIN_EXPORT_PASSWORD,
  RADIO_TEXT_LIMIT,
  type Account,
  type Chat,
  type Contact,
  type Core,
  type CoreEvent,
  type CoreListener,
  type Invitation,
  type Message,
  type PairedRadio,
  type RadioPort,
  type RadioPreset,
  type RadioStatus,
  type Relay,
  type SentInvite,
} from "./types";

// A stand-in for the Rust core with demo data: in the browser preview, and in the app when it is
// started with `VITE_CORE=mock`. It creates no keys and uses no network. It emits the same events
// as the real core, so the interface does not know which one it talks to.

const STORAGE_KEY = "peoples-chat.mock-core";
// 2: chats have a reach, and the demo has a chat reached by radio (#48).
const STORAGE_VERSION = 2;
const MINUTE = 60_000;

/** Demo invite links. The real core's are `nostr:nprofile1…` URIs (#19). */
const INVITE_PREFIX = "peopleschat:demo-invite/";
/** The mock takes anything with a URI scheme as an invite link. */
const LINK_PATTERN = /^[a-z][a-z0-9+.-]*:\S+$/i;

const DEMO_CONTACTS = ["Jonas Kjær", "Sofie Lund", "Lars Bak", "Mikkel Østergaard"];
const DEMO_REPLIES = ["Got it.", "Sounds good.", "Thanks, see you then.", "Noted. Talk later."];

/** MeshCore's EU/UK (Narrow) preset, which the app expects (#24). */
const EU_NARROW: RadioPreset = {
  frequencyKhz: 869_618,
  bandwidthHz: 62_500,
  spreadingFactor: 8,
  codingRate: 8,
  expected: true,
};

/**
 * The demo's USB ports (#24): a radio on the preset the app expects, one on another preset, and a
 * device that isn't a radio.
 */
const DEMO_PORTS: Array<RadioPort & { radio?: { name: string; preset: RadioPreset } }> = [
  { path: "/dev/cu.usbmodem1101", description: "MeshCore radio", radio: { name: "Your radio", preset: EU_NARROW } },
  {
    path: "/dev/cu.usbmodem2101",
    description: "MeshCore radio",
    radio: {
      name: "Spare radio",
      preset: { frequencyKhz: 869_525, bandwidthHz: 250_000, spreadingFactor: 11, codingRate: 5, expected: false },
    },
  },
  { path: "/dev/cu.usbserial-0001", description: "USB serial adapter" },
];

type StorageLike = Pick<Storage, "getItem" | "setItem">;

export interface MockCoreOptions {
  /** Keeps the demo state between reloads. `null` keeps it in memory. Defaults to `localStorage`. */
  storage?: StorageLike | null;
  now?: () => number;
  /**
   * Milliseconds until a relay "acknowledges" a message, and until the contact replies after that;
   * and until a radio message that no radio confirms times out.
   */
  delays?: { sent: number; reply: number; timedOut?: number; invitation?: number };
  /** Whether the contacts' radios confirm radio messages. Without, they time out (#41). Default true. */
  radioConfirms?: boolean;
}

interface MockState {
  account: Account | null;
  chats: Chat[];
  invitations: Invitation[];
  invites: SentInvite[];
  /** Every message goes by radio: the switch for testing (#40). */
  forceRadio: boolean;
  /** The radio paired on one of the demo's ports (#24). */
  pairedRadio: PairedRadio | null;
  /** Invite links already used, and the chat each one opened. */
  links: Record<string, string>;
  demoContactsAdded: number;
  repliesSent: number;
}

export function createMockCore(options: MockCoreOptions = {}): Core {
  const storage = options.storage === undefined ? browserStorage() : options.storage;
  const now = options.now ?? Date.now;
  const delays = options.delays ?? { sent: 600, reply: 1500 };
  // The core waits for the radio's estimate and a margin, then once more after a retry.
  const timedOutDelay = delays.timedOut ?? 12_000;
  // How long a new demo contact's invitation takes to arrive (#126).
  const invitationDelay = delays.invitation ?? 15_000;
  const radioConfirms = options.radioConfirms ?? true;
  const listeners = new Set<CoreListener>();
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const replyTimers = new Map<string, ReturnType<typeof setTimeout>>();
  const state = restore(storage) ?? emptyState();
  // Who each previewed invite link is from, so the chat it starts is with that person (#95).
  const previewed = new Map<string, Contact>();

  // Runs `task` later, unless the account is deleted first.
  function later(ms: number, task: () => void) {
    const timer = setTimeout(() => {
      timers.delete(timer);
      task();
    }, ms);
    timers.add(timer);
    return timer;
  }

  function emit(...events: CoreEvent[]) {
    save(storage, state);
    for (const event of events) {
      // Listeners get copies, as they would from the real core across the Tauri boundary.
      for (const listener of listeners) listener(structuredClone(event));
    }
  }

  function requireAccount(): Account {
    if (!state.account) throw new CoreError("Create an account first.");
    return state.account;
  }

  function chatById(chatId: string): Chat {
    const chat = state.chats.find((c) => c.id === chatId);
    if (!chat) throw new CoreError("This chat no longer exists.");
    return chat;
  }

  // The demo can pair a radio on its USB ports, as a computer can (#24).
  function radioStatus(): RadioStatus {
    return {
      ...(state.pairedRadio ? { paired: state.pairedRadio } : {}),
      forced: state.forceRadio,
      canPair: true,
    };
  }

  function radioChanged(): RadioStatus {
    const radio = radioStatus();
    emit({ type: "radioChanged", radio });
    return structuredClone(radio);
  }

  // The demo contact answers once the user stops sending for a moment.
  function scheduleReply(chatId: string) {
    const pending = replyTimers.get(chatId);
    if (pending) {
      clearTimeout(pending);
      timers.delete(pending);
    }
    const timer = later(delays.reply, () => {
      replyTimers.delete(chatId);
      const chat = state.chats.find((c) => c.id === chatId);
      if (!chat) return;
      const message: Message = {
        id: randomHex(8),
        direction: "in",
        text: DEMO_REPLIES[state.repliesSent % DEMO_REPLIES.length],
        sentAt: now(),
        transport: chat.reach === "radio" ? "radio" : "internet",
      };
      state.repliesSent += 1;
      chat.messages.push(message);
      emit({ type: "messageAdded", chatId, message });
    });
    replyTimers.set(chatId, timer);
  }

  return {
    async load() {
      return structuredClone({
        account: state.account,
        chats: state.chats,
        invitations: state.invitations,
        relays: state.account ? demoRelays() : [],
        invites: state.invites,
        radio: radioStatus(),
      });
    },

    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },

    async createAccount(name) {
      const trimmed = checkedName(name);
      if (state.account) throw new CoreError("This device already has an account.");

      const invite = randomHex(6);
      const account: Account = {
        name: trimmed,
        about: "",
        publicKey: randomHex(32),
        npub: demoNpub(),
        inviteLink: INVITE_PREFIX + invite,
        shareLink: `https://peoples.chat/i#demo-invite-${invite}`,
      };
      const chat = demoChat(now());
      const radioChat = demoRadioChat(now());
      const invitation = demoInvitation(now(), trimmed.split(/\s+/)[0]);
      state.account = account;
      state.chats.push(chat, radioChat);
      state.invitations.push(invitation);
      emit(
        { type: "accountCreated", account },
        { type: "relaysChanged", relays: demoRelays() },
        { type: "chatAdded", chat },
        { type: "chatAdded", chat: radioChat },
        { type: "invitationReceived", invitation },
      );
      return structuredClone(account);
    },

    async updateProfile(name, about) {
      const trimmedName = checkedName(name);
      const trimmedAbout = about.trim();
      if (Array.from(trimmedAbout).length > MAX_ABOUT) {
        throw new CoreError(`About can have up to ${MAX_ABOUT} characters.`);
      }
      const account = requireAccount();
      if (account.name === trimmedName && account.about === trimmedAbout) return structuredClone(account);
      state.account = { ...account, name: trimmedName, about: trimmedAbout };
      emit({ type: "accountChanged", account: state.account });
      return structuredClone(state.account);
    },

    // The demo shows the photo the interface prepared, as it is.
    async setProfilePhoto(photo) {
      const account = requireAccount();
      if (photo.length === 0) throw new CoreError("This photo can't be used. Try another one.");
      state.account = { ...account, photo: `data:image/jpeg;base64,${toBase64(photo)}` };
      emit({ type: "accountChanged", account: state.account });
      return structuredClone(state.account);
    },

    async removeProfilePhoto() {
      const account = requireAccount();
      if (!account.photo) return structuredClone(account);
      state.account = { ...account };
      delete state.account.photo;
      emit({ type: "accountChanged", account: state.account });
      return structuredClone(state.account);
    },

    async deleteAccount() {
      requireAccount();
      for (const timer of timers) clearTimeout(timer);
      timers.clear();
      replyTimers.clear();
      Object.assign(state, emptyState());
      emit({ type: "accountDeleted" });
      // As the core does: the paired radio went with the account, and pairing stays possible.
      radioChanged();
    },

    async addContact(inviteLink) {
      const account = requireAccount();
      const link = inviteLink.trim();
      if (!LINK_PATTERN.test(link)) {
        throw new CoreError("Finding people by name is not possible yet. Paste an invite link instead.");
      }
      if (link === account.inviteLink) throw new CoreError("That is your own invite link.");

      const known = state.chats.find((c) => c.id === state.links[link]);
      if (known) return structuredClone(known);

      let contact = previewed.get(link);
      if (!contact) {
        const round = Math.floor(state.demoContactsAdded / DEMO_CONTACTS.length);
        const baseName = DEMO_CONTACTS[state.demoContactsAdded % DEMO_CONTACTS.length];
        contact = { publicKey: randomHex(32), name: round === 0 ? baseName : `${baseName} ${round + 1}` };
        state.demoContactsAdded += 1;
      }
      const chat: Chat = { id: randomHex(8), contact, messages: [], reach: "internet" };
      state.chats.push(chat);
      state.links[link] = chat.id;
      emit({ type: "chatAdded", chat });
      // The demo's invitations don't arrive at first, and do a while later, to show how a chat
      // says so (#126).
      later(delays.sent, () => {
        const started = state.chats.find((c) => c.id === chat.id);
        if (!started) return;
        started.undelivered = { since: now() };
        emit({ type: "invitationDeliveryChanged", chatId: chat.id, undelivered: started.undelivered });
      });
      later(invitationDelay, () => {
        const started = state.chats.find((c) => c.id === chat.id);
        if (!started?.undelivered) return;
        delete started.undelivered;
        emit({ type: "invitationDeliveryChanged", chatId: chat.id });
      });
      return structuredClone(chat);
    },

    // The demo's links open nothing; the real core's are https://peoples.chat/i links (#95).
    async createInvite(name) {
      const account = requireAccount();
      const trimmed = name.trim();
      if (!trimmed) throw new CoreError("Enter a name.");
      const id = randomHex(10);
      const invite = {
        id,
        name: trimmed,
        link: `${account.shareLink}/${id}`,
        createdAt: now(),
        expiresAt: now() + 7 * 24 * 60 * MINUTE,
      };
      state.invites.push(invite);
      emit({ type: "inviteChanged", invite });
      return structuredClone(invite);
    },

    async previewInvite(inviteLink) {
      const link = inviteLink.trim();
      if (!LINK_PATTERN.test(link)) throw new CoreError("That is not an invite link.");
      const contact = previewed.get(link) ?? { publicKey: randomHex(32), name: "Emil Dahl" };
      previewed.set(link, contact);
      return structuredClone(contact);
    },

    async acceptInvitation(invitationId) {
      requireAccount();
      const invitation = state.invitations.find((i) => i.id === invitationId);
      if (!invitation) throw new CoreError("This request is no longer available.");

      const chat: Chat = { id: randomHex(8), contact: invitation.from, messages: invitation.messages, reach: "internet" };
      state.invitations = state.invitations.filter((i) => i.id !== invitationId);
      state.chats.push(chat);
      emit({ type: "invitationRemoved", invitationId }, { type: "chatAdded", chat });
      return structuredClone(chat);
    },

    async sendMessage(chatId, text) {
      const chat = chatById(chatId);
      const body = text.trim();
      if (!body) throw new CoreError("Write a message first.");
      // As the core does: nothing goes to a chat that has ended (#125).
      if (chat.state === "contactLeft") {
        throw new CoreError(`${chat.contact.name} left this chat. Messages can't be sent here any more.`);
      }
      if (chat.state === "removed") throw new CoreError("You are no longer in this chat.");

      // As the core does: radio carries short text, also when it is forced (#40). With the
      // switch on, longer text is refused; otherwise it waits for the internet (#46), as
      // everything does in a chat that can't be reached.
      const tooLong = radioBytes(body) > RADIO_TEXT_LIMIT;
      if (state.forceRadio && tooLong) {
        throw new CoreError(`That is too long for radio. Keep it to ${RADIO_TEXT_LIMIT} bytes.`);
      }
      const radio = state.forceRadio || (chat.reach === "radio" && !tooLong);
      const waits = !radio && chat.reach !== "internet";
      const message: Message = {
        id: waits ? `queued-${randomHex(8)}` : randomHex(8),
        direction: "out",
        text: body,
        sentAt: now(),
        transport: radio ? "radio" : "internet",
        delivery: "waiting",
        ...(waits ? { cancellable: true } : {}),
        // Its time starts when it goes (#124).
        ...(!waits && chat.disappearAfter ? { expiresAt: now() + chat.disappearAfter * 1000 } : {}),
      };
      chat.messages.push(message);
      emit({ type: "messageAdded", chatId, message });
      if (waits) return structuredClone(message);

      if (radio && !radioConfirms) {
        // No radio answers, also not to the retry: the message times out, and nobody replies.
        later(timedOutDelay, () => {
          message.delivery = "timedOut";
          emit({ type: "deliveryChanged", chatId, messageId: message.id, delivery: "timedOut" });
        });
        return structuredClone(message);
      }
      later(delays.sent, () => {
        // On radio, the other radio confirms; on the internet, a relay acknowledges.
        const delivery = radio ? "confirmed" : "sent";
        message.delivery = delivery;
        emit({ type: "deliveryChanged", chatId, messageId: message.id, delivery });
        scheduleReply(chatId);
      });
      return structuredClone(message);
    },

    async cancelMessage(chatId, messageId) {
      requireAccount();
      const chat = chatById(chatId);
      const message = chat.messages.find((m) => m.id === messageId);
      if (!message) throw new CoreError("This message no longer exists.");
      if (!message.cancellable) throw new CoreError("This message is on its way already.");
      chat.messages = chat.messages.filter((m) => m.id !== messageId);
      emit({ type: "messageRemoved", chatId, messageId });
    },

    // The demo keeps no read marks: its messages disappear after a week, read or not.
    async markRead(chatId) {
      requireAccount();
      chatById(chatId);
    },

    // The demo's radio reaches every contact, so the switch is all it takes.
    async setForceRadio(forced) {
      state.forceRadio = forced;
      return radioChanged();
    },

    async radioPorts() {
      return DEMO_PORTS.map(({ path, description }) => ({ path, description }));
    },

    // As the core says it, for a port that went and for a device that isn't a radio.
    async pairRadio(path) {
      const port = DEMO_PORTS.find((p) => p.path === path);
      if (!port) throw new CoreError("That port is gone. Plug the radio in and look again.");
      if (!port.radio) {
        throw new CoreError(
          `The device on ${path} didn't answer. Is it a MeshCore radio with companion firmware for USB?`,
        );
      }
      if (state.pairedRadio?.port === path) return radioStatus();
      state.pairedRadio = {
        name: port.radio.name,
        publicKey: randomHex(32),
        connection: "connected",
        port: path,
        preset: port.radio.preset,
      };
      return radioChanged();
    },

    async forgetRadio() {
      if (!state.pairedRadio) return radioStatus();
      state.pairedRadio = null;
      return radioChanged();
    },

    // The demo has no real key: these look like the real core's copies, and open nowhere.
    async exportAccountKey(copy) {
      requireAccount();
      if (copy.kind === "plain") return `nsec1demo${randomBech32(54)}`;
      if (Array.from(copy.password).length < MIN_EXPORT_PASSWORD) {
        throw new CoreError(`Use a password of at least ${MIN_EXPORT_PASSWORD} characters.`);
      }
      return `ncryptsec1demo${randomBech32(148)}`;
    },
  };
}

/** A name as given, trimmed, or the core's reason it can't be one. */
function checkedName(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) throw new CoreError("Enter a name.");
  if (Array.from(trimmed).length > MAX_NAME) throw new CoreError(`Your name can have up to ${MAX_NAME} characters.`);
  return trimmed;
}

/**
 * The demo's relays, as in the design: one run by the project, and one of the others not
 * reachable. The real core lists the account's relays as they are (#88).
 */
function demoRelays(): Relay[] {
  return [
    { url: "wss://relay.peoples.chat", operator: "peoplesChat", status: "connected" },
    { url: "wss://relay.aarhus.example", operator: "independent", status: "connected" },
    { url: "wss://relay.nordmesh.example", operator: "independent", status: "unreachable" },
  ];
}

function demoChat(now: number): Chat {
  // Astrid turned disappearing messages on, as a White Noise user can (#124).
  const disappearAfter = 7 * 24 * 60 * 60;
  const expires = (sentAt: number) => sentAt + disappearAfter * 1000;
  return {
    id: randomHex(8),
    contact: { publicKey: randomHex(32), name: "Astrid Holm" },
    messages: [
      {
        id: randomHex(8),
        direction: "in",
        text: "Storm warning for tonight. Are you still coming by tomorrow?",
        sentAt: now - 26 * MINUTE,
        transport: "internet",
        expiresAt: expires(now - 26 * MINUTE),
      },
      {
        id: randomHex(8),
        direction: "out",
        text: "Yes, around three. I will bring the spare radio.",
        sentAt: now - 25 * MINUTE,
        transport: "internet",
        delivery: "sent",
        expiresAt: expires(now - 25 * MINUTE),
      },
    ],
    reach: "internet",
    disappearAfter,
  };
}

/** A chat whose contact can be reached by radio only, as in the design's radio chat. */
function demoRadioChat(now: number): Chat {
  return {
    id: randomHex(8),
    contact: { publicKey: randomHex(32), name: "Hanne Vang", meshes: ["meshCore"] },
    messages: [
      {
        id: randomHex(8),
        direction: "in",
        text: "Power is out on the whole street. Are you okay?",
        sentAt: now - 20 * MINUTE,
        transport: "radio",
      },
      {
        id: randomHex(8),
        direction: "out",
        text: "All fine here. Check on Jonas next door if you can.",
        sentAt: now - 19 * MINUTE,
        transport: "radio",
        delivery: "confirmed",
      },
    ],
    reach: "radio",
  };
}

function demoInvitation(now: number, firstName: string): Invitation {
  const receivedAt = now - 4 * MINUTE;
  return {
    id: randomHex(8),
    from: { publicKey: randomHex(32), name: "Karen Winther" },
    receivedAt,
    messages: [
      {
        id: randomHex(8),
        direction: "in",
        text: `Hi ${firstName}, Astrid gave me your link. Can we keep in touch here?`,
        sentAt: receivedAt,
        transport: "internet",
      },
    ],
  };
}

function emptyState(): MockState {
  return {
    account: null,
    chats: [],
    invitations: [],
    invites: [],
    forceRadio: false,
    pairedRadio: null,
    links: {},
    demoContactsAdded: 0,
    repliesSent: 0,
  };
}

function browserStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function save(storage: StorageLike | null, state: MockState) {
  try {
    storage?.setItem(STORAGE_KEY, JSON.stringify({ v: STORAGE_VERSION, state }));
  } catch {
    // Storage full or blocked: the demo keeps running in memory.
  }
}

function restore(storage: StorageLike | null): MockState | null {
  try {
    const raw = storage?.getItem(STORAGE_KEY);
    if (!raw) return null;
    const saved = JSON.parse(raw) as { v?: unknown; state?: Partial<MockState> };
    const state = saved.state;
    if (saved.v !== STORAGE_VERSION || !state || !Array.isArray(state.chats) || !Array.isArray(state.invitations)) {
      return null;
    }
    const restored = { ...emptyState(), ...state } as MockState;
    // Saved before profiles had an about (#93) and accounts a web link (#72).
    if (restored.account && typeof restored.account.about !== "string") restored.account.about = "";
    if (restored.account && typeof restored.account.shareLink !== "string") {
      restored.account.shareLink = `https://peoples.chat/i#${restored.account.inviteLink.replace(INVITE_PREFIX, "demo-invite-")}`;
    }
    // Saved before accounts showed their npub (#129).
    if (restored.account && typeof restored.account.npub !== "string") restored.account.npub = demoNpub();
    // Acknowledgement timers do not survive a reload, so settle anything still waiting, except
    // in a chat that can't be reached.
    for (const chat of restored.chats) {
      if (chat.reach === "unreachable") continue;
      for (const message of chat.messages) {
        if (message.delivery === "waiting") message.delivery = message.transport === "radio" ? "confirmed" : "sent";
      }
    }
    return restored;
  } catch {
    return null;
  }
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function randomBech32(length: number): string {
  const alphabet = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
  return Array.from(crypto.getRandomValues(new Uint8Array(length)), (b) => alphabet[b % 32]).join("");
}

/** Looks like an npub, but is no key: the demo has none (#129). */
function demoNpub(): string {
  const bech32 = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
  return "npub1" + Array.from(crypto.getRandomValues(new Uint8Array(58)), (b) => bech32[b % 32]).join("");
}

function randomHex(bytes: number): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) => b.toString(16).padStart(2, "0")).join("");
}
