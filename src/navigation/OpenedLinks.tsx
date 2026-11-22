import { useEffect, useRef } from "react";
import { followOpenedLinks } from "../platform";
import { useAppState } from "../state";
import { useNavigation } from "./Navigator";

/**
 * Opens New chat with each invite link tapped in another app, filled in for the user to confirm
 * (#72). A link that arrives before there is an account waits until there is one.
 */
export function OpenedLinks() {
  const navigation = useNavigation();
  const { account } = useAppState();
  const latest = useRef({ navigation, hasAccount: account !== null });
  latest.current = { navigation, hasAccount: account !== null };
  const waiting = useRef<string | null>(null);

  useEffect(() => {
    // An effect that has ended opens nothing. Without this, development builds open the link that
    // started the app twice, since React's StrictMode runs the effect twice.
    let active = true;
    function open(link: string) {
      if (!active) return;
      const { navigation, hasAccount } = latest.current;
      if (!hasAccount) {
        waiting.current = link;
        return;
      }
      const screen = { name: "newChat" as const, link };
      if (navigation.current.name === "newChat") navigation.replace(screen);
      else navigation.push(screen);
    }

    let stop: (() => void) | null = null;
    followOpenedLinks(open).then(
      (unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      },
      (error) => console.warn("Links that open the app are not followed:", error),
    );
    return () => {
      active = false;
      stop?.();
    };
  }, []);

  // Once the account exists and the chat list shows, the link that waited opens. Creating the
  // account resets the screens to the chat list, which would drop a screen opened earlier.
  useEffect(() => {
    if (account === null || waiting.current === null || navigation.current.name !== "chats") return;
    const link = waiting.current;
    waiting.current = null;
    navigation.push({ name: "newChat", link });
  }, [account, navigation]);

  return null;
}
