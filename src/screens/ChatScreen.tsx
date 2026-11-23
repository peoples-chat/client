import { useState } from "react";
import { core } from "../core";
import type { Contact, Reach } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { Avatar } from "../ui/Avatar";
import { IconButton } from "../ui/Button";
import { Composer } from "../ui/Composer";
import { Icon } from "../ui/Icon";
import { MessageList } from "../ui/MessageList";
import { firstName, tintFor } from "../ui/people";
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
 *
 * A chat without messages introduces the contact. Opened right after joining from their invite
 * link (`invited`), it says who invited the user and suggests a first message (#95).
 */
export function ChatScreen({ chatId, invited = false }: { chatId: string; invited?: boolean }) {
  const { chats, invites } = useAppState();
  const navigation = useNavigation();
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const chat = chats.find((c) => c.id === chatId);

  if (!chat) return <Missing title="Chat" text="This chat no longer exists." />;
  const name = chat.contact.name;
  const first = firstName(name);
  // Snapshots from before #43 have no reach; their chats were on the internet.
  const reach: Reach = chat.reach ?? "internet";
  const joined = invites.some((invite) => invite.joinedChatId === chatId);
  const suggestions =
    invited && chat.messages.length === 0 ? [`Hi ${first}, I made it`, "What is the radio part?"] : [];

  async function send(text: string) {
    setError(null);
    try {
      await core.sendMessage(chatId, text);
    } catch (e) {
      setError(errorText(e));
      throw e;
    }
  }

  async function sendSuggestion(text: string) {
    setSending(true);
    try {
      await send(text);
    } catch {
      // The screen shows why.
    } finally {
      setSending(false);
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
      <MessageList
        messages={chat.messages}
        empty={
          <ChatIntro
            contact={chat.contact}
            line={
              invited
                ? `${first} invited you to peoples.chat`
                : joined
                  ? "Joined from your invite. Say hi."
                  : "No messages yet. Say hello."
            }
          />
        }
      />
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
      {suggestions.length > 0 && (
        <div className={styles.suggestions} aria-label="Suggested messages">
          {suggestions.map((text) => (
            <button
              key={text}
              type="button"
              className={styles.suggestion}
              disabled={sending}
              onClick={() => sendSuggestion(text)}
            >
              {text}
            </button>
          ))}
        </div>
      )}
      <Composer onSend={send} mode={reach === "radio" ? "radio" : "internet"} />
    </div>
  );
}

/** The start of a chat without messages: who it is with, and that it is end-to-end encrypted. */
function ChatIntro({ contact, line }: { contact: Contact; line: string }) {
  return (
    <div className={styles.intro}>
      <div className={styles.introPerson}>
        <Avatar name={contact.name} size={76} tint={tintFor(contact.name)} photo={contact.photo} />
        <p className={styles.introName}>{contact.name}</p>
        <p className={styles.introLine}>{line}</p>
      </div>
      <p className={styles.encrypted}>
        <Icon name="lock" size={14} strokeWidth={2.2} />
        Messages in this chat are end-to-end encrypted.
      </p>
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
