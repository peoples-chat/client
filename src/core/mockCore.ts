import {
  CoreError,
  type Account,
  type Chat,
  type Core,
  type CoreEvent,
  type CoreListener,
  type Invitation,
  type Message,
} from "./types";

// A stand-in for the Rust core, so the interface can be built and tried before the real core
// lands (#17, #19, #20). It holds demo data, creates no keys and uses no network. It emits the
// same events the real core will, so the interface does not know which one it talks to.

const STORAGE_KEY = "peoples-chat.mock-core";
const STORAGE_VERSION = 1;
const MINUTE = 60_000;

/** Demo invite links. The real link format comes with #19. */
const INVITE_PREFIX = "peopleschat:demo-invite/";
/** The mock takes anything with a URI scheme as an invite link. */
const LINK_PATTERN = /^[a-z][a-z0-9+.-]*:\S+$/i;

const DEMO_CONTACTS = ["Jonas Kjær", "Sofie Lund", "Lars Bak", "Mikkel Østergaard"];
const DEMO_REPLIES = ["Got it.", "Sounds good.", "Thanks, see you then.", "Noted. Talk later."];

type StorageLike = Pick<Storage, "getItem" | "setItem">;

export interface MockCoreOptions {
  /** Keeps the demo state between reloads. `null` keeps it in memory. Defaults to `localStorage`. */
  storage?: StorageLike | null;
  now?: () => number;
  /** Milliseconds until a relay "acknowledges" a message, and until the contact replies after that. */
  delays?: { sent: number; reply: number };
}

interface MockState {
  account: Account | null;
  chats: Chat[];
  invitations: Invitation[];
  /** Invite links already used, and the chat each one opened. */
  links: Record<string, string>;
  demoContactsAdded: number;
  repliesSent: number;
}

export function createMockCore(options: MockCoreOptions = {}): Core {
  const storage = options.storage === undefined ? browserStorage() : options.storage;
  const now = options.now ?? Date.now;
  const delays = options.delays ?? { sent: 600, reply: 1500 };
  const listeners = new Set<CoreListener>();
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const replyTimers = new Map<string, ReturnType<typeof setTimeout>>();
  const state = restore(storage) ?? emptyState();

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
        transport: "internet",
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
      });
    },

    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },

    async createAccount(name) {
      const trimmed = name.trim();
      if (!trimmed) throw new CoreError("Enter a name.");
      if (state.account) throw new CoreError("This device already has an account.");

      const account: Account = {
        name: trimmed,
        publicKey: randomHex(32),
        inviteLink: INVITE_PREFIX + randomHex(6),
      };
      const chat = demoChat(now());
      const invitation = demoInvitation(now(), trimmed.split(/\s+/)[0]);
      state.account = account;
      state.chats.push(chat);
      state.invitations.push(invitation);
      emit(
        { type: "accountCreated", account },
        { type: "chatAdded", chat },
        { type: "invitationReceived", invitation },
      );
      return structuredClone(account);
    },

    async deleteAccount() {
      requireAccount();
      for (const timer of timers) clearTimeout(timer);
      timers.clear();
      replyTimers.clear();
      Object.assign(state, emptyState());
      emit({ type: "accountDeleted" });
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

      const round = Math.floor(state.demoContactsAdded / DEMO_CONTACTS.length);
      const baseName = DEMO_CONTACTS[state.demoContactsAdded % DEMO_CONTACTS.length];
      const chat: Chat = {
        id: randomHex(8),
        contact: { publicKey: randomHex(32), name: round === 0 ? baseName : `${baseName} ${round + 1}` },
        messages: [],
      };
      state.demoContactsAdded += 1;
      state.chats.push(chat);
      state.links[link] = chat.id;
      emit({ type: "chatAdded", chat });
      return structuredClone(chat);
    },

    async acceptInvitation(invitationId) {
      requireAccount();
      const invitation = state.invitations.find((i) => i.id === invitationId);
      if (!invitation) throw new CoreError("This request is no longer available.");

      const chat: Chat = { id: randomHex(8), contact: invitation.from, messages: invitation.messages };
      state.invitations = state.invitations.filter((i) => i.id !== invitationId);
      state.chats.push(chat);
      emit({ type: "invitationRemoved", invitationId }, { type: "chatAdded", chat });
      return structuredClone(chat);
    },

    async sendMessage(chatId, text) {
      const chat = chatById(chatId);
      const body = text.trim();
      if (!body) throw new CoreError("Write a message first.");

      const message: Message = {
        id: randomHex(8),
        direction: "out",
        text: body,
        sentAt: now(),
        transport: "internet",
        delivery: "waiting",
      };
      chat.messages.push(message);
      emit({ type: "messageAdded", chatId, message });

      later(delays.sent, () => {
        message.delivery = "sent";
        emit({ type: "deliveryChanged", chatId, messageId: message.id, delivery: "sent" });
        scheduleReply(chatId);
      });
      return structuredClone(message);
    },
  };
}

function demoChat(now: number): Chat {
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
      },
      {
        id: randomHex(8),
        direction: "out",
        text: "Yes, around three. I will bring the spare radio.",
        sentAt: now - 25 * MINUTE,
        transport: "internet",
        delivery: "sent",
      },
    ],
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
  return { account: null, chats: [], invitations: [], links: {}, demoContactsAdded: 0, repliesSent: 0 };
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
    // Acknowledgement timers do not survive a reload, so settle anything still waiting.
    for (const chat of restored.chats) {
      for (const message of chat.messages) {
        if (message.delivery === "waiting") message.delivery = "sent";
      }
    }
    return restored;
  } catch {
    return null;
  }
}

function randomHex(bytes: number): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) => b.toString(16).padStart(2, "0")).join("");
}
