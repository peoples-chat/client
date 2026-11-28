import type {
  Account,
  Chat,
  Contact,
  Core,
  CoreEvent,
  Invitation,
  Message,
  RadioPreset,
  RadioStatus,
  Relay,
  SentInvite,
  Snapshot,
} from "../core/types";

/** What the interface shows. It changes only through core events. */
export interface AppState {
  /** `loading` until the core's first snapshot has arrived. */
  status: "loading" | "ready" | "failed";
  account: Account | null;
  chats: Chat[];
  invitations: Invitation[];
  relays: Relay[];
  invites: SentInvite[];
  radio: RadioStatus;
  /** Why loading failed. */
  error?: string;
}

const NO_RADIO: RadioStatus = { forced: false };

export const initialState: AppState = {
  status: "loading",
  account: null,
  chats: [],
  invitations: [],
  relays: [],
  invites: [],
  radio: NO_RADIO,
};

export function fromSnapshot(snapshot: Snapshot): AppState {
  return {
    status: "ready",
    account: snapshot.account,
    chats: snapshot.chats,
    invitations: snapshot.invitations,
    relays: snapshot.relays,
    invites: snapshot.invites,
    // Snapshots from before #40 have no radio.
    radio: snapshot.radio ?? NO_RADIO,
  };
}

/**
 * Applies one core event. Applying an event twice changes nothing, because the store can see an
 * event and also a snapshot that already contains it.
 */
export function applyEvent(state: AppState, event: CoreEvent): AppState {
  switch (event.type) {
    case "accountCreated":
      return { ...state, account: event.account };

    case "accountDeleted":
      // The core forgets the radio with the account. Whether the device can pair one follows
      // as `radioChanged` (#24).
      return { ...state, account: null, chats: [], invitations: [], relays: [], invites: [], radio: NO_RADIO };

    case "chatAdded":
      if (state.chats.some((c) => c.id === event.chat.id)) return state;
      return { ...state, chats: [...state.chats, event.chat] };

    case "chatChanged":
      // The whole chat replaces the one with its id (#125).
      return updateChat(state, event.chat.id, (chat) => (sameChat(chat, event.chat) ? chat : event.chat));

    case "invitationReceived":
      if (state.invitations.some((i) => i.id === event.invitation.id)) return state;
      return { ...state, invitations: [...state.invitations, event.invitation] };

    case "invitationRemoved":
      if (!state.invitations.some((i) => i.id === event.invitationId)) return state;
      return { ...state, invitations: state.invitations.filter((i) => i.id !== event.invitationId) };

    case "messageAdded":
      // The chat id can be an invitation's: messages that arrive before the user accepts.
      if (!state.chats.some((c) => c.id === event.chatId)) return addToInvitation(state, event.chatId, event.message);
      return updateChat(state, event.chatId, (chat) =>
        chat.messages.some((m) => m.id === event.message.id)
          ? chat
          : { ...chat, messages: [...chat.messages, event.message] },
      );

    case "messageRemoved": {
      const without = (messages: Message[]) => messages.filter((m) => m.id !== event.messageId);
      if (state.chats.some((c) => c.id === event.chatId)) {
        return updateChat(state, event.chatId, (chat) =>
          chat.messages.some((m) => m.id === event.messageId) ? { ...chat, messages: without(chat.messages) } : chat,
        );
      }
      // A message that came with an invitation can disappear too (#124).
      const invitation = state.invitations.find((i) => i.id === event.chatId);
      if (!invitation || !invitation.messages.some((m) => m.id === event.messageId)) return state;
      return {
        ...state,
        invitations: state.invitations.map((i) => (i === invitation ? { ...i, messages: without(i.messages) } : i)),
      };
    }

    case "messageChanged": {
      const replace = (messages: Message[]) => {
        const known = messages.find((m) => m.id === event.message.id);
        if (!known || sameMessage(known, event.message)) return messages;
        return messages.map((m) => (m.id === event.message.id ? event.message : m));
      };
      if (state.chats.some((c) => c.id === event.chatId)) {
        return updateChat(state, event.chatId, (chat) => {
          const messages = replace(chat.messages);
          return messages === chat.messages ? chat : { ...chat, messages };
        });
      }
      // Messages that came before the user accepted an invitation change too.
      const invitation = state.invitations.find((i) => i.id === event.chatId);
      if (!invitation) return state;
      const messages = replace(invitation.messages);
      if (messages === invitation.messages) return state;
      return { ...state, invitations: state.invitations.map((i) => (i === invitation ? { ...i, messages } : i)) };
    }

    case "disappearingChanged":
      return updateChat(state, event.chatId, (chat) =>
        chat.disappearAfter === event.disappearAfter ? chat : { ...chat, disappearAfter: event.disappearAfter },
      );

    case "deliveryChanged":
      return updateChat(state, event.chatId, (chat) => {
        const message = chat.messages.find((m) => m.id === event.messageId);
        if (!message || message.delivery === event.delivery) return chat;
        return {
          ...chat,
          messages: chat.messages.map((m) => (m.id === event.messageId ? { ...m, delivery: event.delivery } : m)),
        };
      });

    case "reachChanged":
      return updateChat(state, event.chatId, (chat) =>
        chat.reach === event.reach && chat.radioIssue === event.radioIssue
          ? chat
          : { ...chat, reach: event.reach, radioIssue: event.radioIssue },
      );

    case "relaysChanged":
      return sameRelays(state.relays, event.relays) ? state : { ...state, relays: event.relays };

    case "accountChanged":
      if (!state.account || sameAccount(state.account, event.account)) return state;
      return { ...state, account: event.account };

    case "contactChanged":
      return updateContact(state, event.contact);

    case "radioChanged":
      return sameRadio(state.radio, event.radio) ? state : { ...state, radio: event.radio };

    case "inviteChanged": {
      const known = state.invites.find((i) => i.id === event.invite.id);
      if (known && known.joinedChatId === event.invite.joinedChatId && known.link === event.invite.link) return state;
      return known
        ? { ...state, invites: state.invites.map((i) => (i.id === event.invite.id ? event.invite : i)) }
        : { ...state, invites: [...state.invites, event.invite] };
    }
  }
}

function sameMessage(a: Message, b: Message): boolean {
  return (
    a.id === b.id &&
    a.direction === b.direction &&
    a.text === b.text &&
    a.sentAt === b.sentAt &&
    a.transport === b.transport &&
    a.delivery === b.delivery &&
    (a.cancellable ?? false) === (b.cancellable ?? false) &&
    (a.edited ?? false) === (b.edited ?? false) &&
    (a.deleted ?? false) === (b.deleted ?? false) &&
    (a.withdrawn ?? false) === (b.withdrawn ?? false) &&
    a.sender?.publicKey === b.sender?.publicKey &&
    a.sender?.name === b.sender?.name
  );
}

/** Whether two chats show the same. Both come from the core as JSON. */
function sameChat(a: Chat, b: Chat): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Whether messages can still be sent in the chat (#125). */
export function canSend(chat: Chat): boolean {
  return chat.state !== "contactLeft" && chat.state !== "removed";
}

/**
 * What the chat says once it is no longer a direct chat (#125), or `null` while it is one.
 * `first` is the contact's first name.
 */
export function chatEndedText(chat: Chat, first: string): string | null {
  switch (chat.state ?? "direct") {
    case "direct":
      return null;
    case "contactLeft":
      return `${first} left this chat. Messages can't be sent here any more.`;
    case "removed":
      return "You are no longer in this chat. Messages can't be sent here any more.";
    case "group":
      return "This is no longer a direct chat: it has a name or more people now.";
  }
}

function sameAccount(a: Account, b: Account): boolean {
  return (
    a.name === b.name &&
    a.about === b.about &&
    a.photo === b.photo &&
    a.publicKey === b.publicKey &&
    a.inviteLink === b.inviteLink &&
    a.shareLink === b.shareLink
  );
}

/** Gives the contact their new name, photo and meshes in their chats and invitations. */
function updateContact(state: AppState, contact: Contact): AppState {
  const outdated = (current: Contact) =>
    current.publicKey === contact.publicKey &&
    (current.name !== contact.name ||
      current.photo !== contact.photo ||
      (current.meshes ?? []).join() !== (contact.meshes ?? []).join());
  if (!state.chats.some((c) => outdated(c.contact)) && !state.invitations.some((i) => outdated(i.from))) return state;
  return {
    ...state,
    chats: state.chats.map((c) => (outdated(c.contact) ? { ...c, contact } : c)),
    invitations: state.invitations.map((i) => (outdated(i.from) ? { ...i, from: contact } : i)),
  };
}

function sameRadio(a: RadioStatus, b: RadioStatus): boolean {
  return (
    a.forced === b.forced &&
    (a.canPair ?? false) === (b.canPair ?? false) &&
    a.paired?.name === b.paired?.name &&
    a.paired?.publicKey === b.paired?.publicKey &&
    a.paired?.connection === b.paired?.connection &&
    a.paired?.port === b.paired?.port &&
    samePreset(a.paired?.preset, b.paired?.preset)
  );
}

function samePreset(a?: RadioPreset, b?: RadioPreset): boolean {
  return (
    a?.frequencyKhz === b?.frequencyKhz &&
    a?.bandwidthHz === b?.bandwidthHz &&
    a?.spreadingFactor === b?.spreadingFactor &&
    a?.codingRate === b?.codingRate &&
    a?.expected === b?.expected
  );
}

function sameRelays(a: Relay[], b: Relay[]): boolean {
  return (
    a.length === b.length &&
    a.every((relay, i) => relay.url === b[i].url && relay.operator === b[i].operator && relay.status === b[i].status)
  );
}

function addToInvitation(state: AppState, invitationId: string, message: Message): AppState {
  const invitation = state.invitations.find((i) => i.id === invitationId);
  if (!invitation || invitation.messages.some((m) => m.id === message.id)) return state;
  return {
    ...state,
    invitations: state.invitations.map((i) => (i === invitation ? { ...i, messages: [...i.messages, message] } : i)),
  };
}

function updateChat(state: AppState, chatId: string, update: (chat: Chat) => Chat): AppState {
  let changed = false;
  const chats = state.chats.map((chat) => {
    if (chat.id !== chatId) return chat;
    const next = update(chat);
    changed = next !== chat;
    return next;
  });
  return changed ? { ...state, chats } : state;
}

export interface Store {
  getState(): AppState;
  subscribe(listener: () => void): () => void;
  /** Loads the first snapshot. Events that arrive before it are applied after it. */
  start(): Promise<void>;
}

export function createStore(core: Core): Store {
  let state = initialState;
  let queued: CoreEvent[] | null = [];
  const listeners = new Set<() => void>();

  function set(next: AppState) {
    if (next === state) return;
    state = next;
    for (const listener of listeners) listener();
  }

  core.subscribe((event) => {
    if (queued) queued.push(event);
    else set(applyEvent(state, event));
  });

  return {
    getState: () => state,

    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },

    async start() {
      try {
        let next = fromSnapshot(await core.load());
        for (const event of queued ?? []) next = applyEvent(next, event);
        queued = null;
        set(next);
      } catch (error) {
        set({ ...state, status: "failed", error: error instanceof Error ? error.message : String(error) });
      }
    },
  };
}

/** Time of a chat's newest message, or 0 for a chat without messages. */
export function lastActivity(chat: Chat): number {
  return chat.messages.length ? chat.messages[chat.messages.length - 1].sentAt : 0;
}

export type ChatListItem =
  | { kind: "chat"; chat: Chat; time: number }
  | { kind: "invitation"; invitation: Invitation; time: number };

/** Message requests and chats for the chat list, newest first. */
export function chatListItems(state: AppState): ChatListItem[] {
  const items: ChatListItem[] = [
    ...state.invitations.map((invitation) => ({ kind: "invitation" as const, invitation, time: invitation.receivedAt })),
    ...state.chats.map((chat) => ({ kind: "chat" as const, chat, time: lastActivity(chat) })),
  ];
  return items.sort((a, b) => b.time - a.time);
}
