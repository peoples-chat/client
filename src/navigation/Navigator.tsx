import { createContext, useContext, useEffect, useMemo, useReducer, useRef, type ReactNode } from "react";
import { screenKey, type Screen } from "./screens";

export interface Navigation {
  /** The screen on top of the stack. */
  current: Screen;
  push(screen: Screen): void;
  /** One screen back, like the system back button. Does nothing on the root screen. */
  back(): void;
  /** Swaps the top screen, for example a request that became a chat. */
  replace(screen: Screen): void;
  /** Replaces the whole stack, for example after first launch or when switching tabs. */
  reset(stack: Screen[]): void;
}

// Every screen on the stack has a browser history entry, so the browser's and Android's back
// buttons pop screens. Each entry records its depth and an id of this page load, so entries
// left over from before a reload are treated as the root.
const PAGE_ID = Math.random().toString(36).slice(2);

interface HistoryEntry {
  page: string;
  depth: number;
}

const entry = (depth: number): HistoryEntry => ({ page: PAGE_ID, depth });

const NavigationContext = createContext<Navigation | null>(null);

export function useNavigation(): Navigation {
  const navigation = useContext(NavigationContext);
  if (!navigation) throw new Error("useNavigation needs a <Navigator> around it");
  return navigation;
}

interface NavigatorProps {
  initial: Screen;
  /** Renders one screen. Screens below the top stay mounted but hidden, so they keep their state. */
  children: (screen: Screen) => ReactNode;
}

export function Navigator({ initial, children }: NavigatorProps) {
  const stackRef = useRef<Screen[]>([initial]);
  const pendingReset = useRef<Screen[] | null>(null);
  const [, rerender] = useReducer((n: number) => n + 1, 0);

  const actions = useMemo(() => {
    const commit = (next: Screen[]) => {
      stackRef.current = next;
      rerender();
    };
    const pushEntries = (stack: Screen[]) => {
      for (let depth = 1; depth < stack.length; depth++) history.pushState(entry(depth), "");
    };

    return {
      commit,
      pushEntries,
      push(screen: Screen) {
        const next = [...stackRef.current, screen];
        history.pushState(entry(next.length - 1), "");
        commit(next);
      },
      back() {
        if (stackRef.current.length > 1) history.back();
      },
      replace(screen: Screen) {
        commit([...stackRef.current.slice(0, -1), screen]);
      },
      reset(next: Screen[]) {
        const depth = stackRef.current.length - 1;
        if (depth === 0) {
          commit(next);
          pushEntries(next);
        } else {
          // Unwind the history to the root first; the popstate handler then applies the reset.
          pendingReset.current = next;
          history.go(-depth);
        }
      },
    };
  }, []);

  useEffect(() => {
    history.replaceState(entry(0), "");

    const onPopState = (event: PopStateEvent) => {
      const state = event.state as Partial<HistoryEntry> | null;
      const ours = state?.page === PAGE_ID && typeof state.depth === "number";
      const depth = ours ? (state.depth as number) : 0;

      const reset = pendingReset.current;
      if (reset) {
        if (depth !== 0) return;
        pendingReset.current = null;
        history.replaceState(entry(0), "");
        actions.commit(reset);
        actions.pushEntries(reset);
        return;
      }

      if (!ours) history.replaceState(entry(0), "");
      const length = Math.min(depth + 1, stackRef.current.length);
      if (length !== stackRef.current.length) actions.commit(stackRef.current.slice(0, length));
    };

    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, [actions]);

  const stack = stackRef.current;
  const top = stack.length - 1;
  const navigation: Navigation = {
    current: stack[top],
    push: actions.push,
    back: actions.back,
    replace: actions.replace,
    reset: actions.reset,
  };

  return (
    <NavigationContext.Provider value={navigation}>
      {stack.map((screen, depth) => (
        <div key={`${depth}:${screenKey(screen)}`} className="screen-slot" hidden={depth !== top}>
          {children(screen)}
        </div>
      ))}
    </NavigationContext.Provider>
  );
}
