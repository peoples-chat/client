import type { Account, Chat, Core, CoreEvent, Invitation, Snapshot } from "../core/types";

/** What the interface shows. It changes only through core events. */
export interface AppState {
  /** `loading` until the core's first snapshot has arrived. */
  status: "loading" | "ready" | "failed";
  account: Account | null;
  chats: Chat[];
  invitations: Invitation[];
  /** Why loading failed. */
  error?: string;
}

export const initialState: AppState = { status: "loading", account: null, chats: [], invitations: [] };

export function fromSnapshot(snapshot: Snapshot): AppState {
  return {
    status: "ready",
    account: snapshot.account,
    chats: snapshot.chats,
    invitations: snapshot.invitations,
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
  }
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
