import { useState } from "react";
import { core } from "../core";
import type { Reach } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { IconButton } from "../ui/Button";
import { Composer } from "../ui/Composer";
import { Icon } from "../ui/Icon";
import { MessageList } from "../ui/MessageList";
import { tintFor } from "../ui/people";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText, Missing } from "./shared";
import styles from "./ChatScreen.module.css";

/**
 * A direct chat. Calls are in the design but not built yet, so their buttons are disabled.
 * The design's presence line ("Online") and read receipts are not part of the plan.
 *
 * When the internet is down, the header says how the contact can be reached, a notice says what
 * happens to messages and leads to what radio protects, and the composer switches to short text
 * by radio (#48).
 */
export function ChatScreen({ chatId }: { chatId: string }) {
  const { chats } = useAppState();
  const navigation = useNavigation();
  const [error, setError] = useState<string | null>(null);
  const chat = chats.find((c) => c.id === chatId);

  if (!chat) return <Missing title="Chat" text="This chat no longer exists." />;
  const name = chat.contact.name;
  // Snapshots from before #43 have no reach; their chats were on the internet.
  const reach: Reach = chat.reach ?? "internet";

  async function send(text: string) {
    setError(null);
    try {
      await core.sendMessage(chatId, text);
    } catch (e) {
      setError(errorText(e));
      throw e;
    }
  }

  return (
    <div className={layout.screen}>
      <ScreenHeader
        title={name}
        subtitle={reach === "internet" ? undefined : <ReachLine reach={reach} />}
        avatar={{ name, tint: tintFor(name), photo: chat.contact.photo }}
        bordered
        onLeading={navigation.back}
        actions={
          <>
            <IconButton label="Start a video call" icon="video" disabled />
            <IconButton label="Start a voice call" icon="phone" disabled />
          </>
        }
      />
      <MessageList messages={chat.messages} empty="No messages yet. Say hello." />
      {reach !== "internet" && (
        <button
          type="button"
          className={`${styles.notice} ${reach === "radio" ? styles.radio : ""}`}
          onClick={() => navigation.push({ name: "radioNotice" })}
        >
          <Icon name={reach === "radio" ? "radio" : "clock"} size={14} strokeWidth={2.2} />
          <span>
            {reach === "radio"
              ? "Internet is down. Messages now go by radio."
              : "Internet is down. Messages wait until it is back."}
          </span>
        </button>
      )}
      {error && (
        <p className={layout.error} role="alert">
          {error}
        </p>
      )}
      <Composer onSend={send} mode={reach === "radio" ? "radio" : "internet"} />
    </div>
  );
}

/** The header's line under the name, when the contact can't be reached over the internet. */
function ReachLine({ reach }: { reach: "radio" | "unreachable" }) {
  if (reach === "unreachable") return <span>Not reachable</span>;
  return (
    <span className={styles.reachRadio}>
      <Icon name="radio" size={14} strokeWidth={2.2} />
      Reachable by radio
    </span>
  );
}
