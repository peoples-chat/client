import { useSyncExternalStore } from "react";
import { core } from "../core";
import { createStore, type AppState } from "./store";

/** The one store of the app, fed by the active core. Started from `main.tsx`. */
export const store = createStore(core);

export function useAppState(): AppState {
  return useSyncExternalStore(store.subscribe, store.getState);
}

export { chatListItems, lastActivity, type AppState, type ChatListItem } from "./store";
