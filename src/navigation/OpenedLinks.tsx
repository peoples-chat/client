import { useEffect, useRef } from "react";
import { followOpenedLinks } from "../platform";
import { useAppState } from "../state";
import { useNavigation } from "./Navigator";

/**
 * Opens each invite link tapped in another app (#72): New chat with the link filled in, for the
 * user to confirm. Without an account, the screen for someone who was invited (#95).
 */
export function OpenedLinks() {
  const navigation = useNavigation();
  const { account } = useAppState();
  const latest = useRef({ navigation, hasAccount: account !== null });
  latest.current = { navigation, hasAccount: account !== null };

  useEffect(() => {
    function open(link: string) {
      const { navigation, hasAccount } = latest.current;
      const screen = hasAccount ? { name: "newChat" as const, link } : { name: "invitedStart" as const, link };
      if (navigation.current.name === screen.name) navigation.replace(screen);
      else navigation.push(screen);
    }

    let stop: (() => void) | null = null;
    let active = true;
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

  return null;
}
