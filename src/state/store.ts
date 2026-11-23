import type { Account, Chat, Contact, Core, CoreEvent, Invitation, Message, Relay, Snapshot } from "../core/types";

/** What the interface shows. It changes only through core events. */
export interface AppState {
  /** `loading` until the core's first snapshot has arrived. */
  status: "loading" | "ready" | "failed";
  account: Account | null;
  chats: Chat[];
  invitations: Invitation[];
  relays: Relay[];
  /** Why loading failed. */
  error?: string;
}

export const initialState: AppState = { status: "loading", account: null, chats: [], invitations: [], relays: [] };

export function fromSnapshot(snapshot: Snapshot): AppState {
  return {
    status: "ready",
    account: snapshot.account,
    chats: snapshot.chats,
    invitations: snapshot.invitations,
    relays: snapshot.relays,
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
      return { ...state, account: null, chats: [], invitations: [], relays: [] };

    case "chatAdded":
      if (state.chats.some((c) => c.id === event.chat.id)) return state;
      return { ...state, chats: [...state.chats, event.chat] };

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
        chat.reach === event.reach ? chat : { ...chat, reach: event.reach },
      );

    case "relaysChanged":
      return sameRelays(state.relays, event.relays) ? state : { ...state, relays: event.relays };

    case "accountChanged":
      if (!state.account || sameAccount(state.account, event.account)) return state;
      return { ...state, account: event.account };

    case "contactChanged":
      return updateContact(state, event.contact);
  }
}

function sameAccount(a: Account, b: Account): boolean {
  return (
    a.name === b.name &&
    a.about === b.about &&
    a.photo === b.photo &&
    a.publicKey === b.publicKey &&
    a.inviteLink === b.inviteLink
  );
}

/** Gives the contact their new name and photo in their chats and invitations. */
function updateContact(state: AppState, contact: Contact): AppState {
  const outdated = (current: Contact) =>
    current.publicKey === contact.publicKey && (current.name !== contact.name || current.photo !== contact.photo);
  if (!state.chats.some((c) => outdated(c.contact)) && !state.invitations.some((i) => outdated(i.from))) return state;
  return {
    ...state,
    chats: state.chats.map((c) => (outdated(c.contact) ? { ...c, contact } : c)),
    invitations: state.invitations.map((i) => (outdated(i.from) ? { ...i, from: contact } : i)),
  };
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
