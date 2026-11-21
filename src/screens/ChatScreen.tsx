import { useState } from "react";
import { core } from "../core";
import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { IconButton } from "../ui/Button";
import { Composer } from "../ui/Composer";
import { MessageList } from "../ui/MessageList";
import { tintFor } from "../ui/people";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText, Missing } from "./shared";

/**
 * A direct chat. Calls are in the design but not built yet, so their buttons are disabled.
 * The design's presence line ("Online") and read receipts are not part of the plan.
 */
export function ChatScreen({ chatId }: { chatId: string }) {
  const { chats } = useAppState();
  const navigation = useNavigation();
  const [error, setError] = useState<string | null>(null);
  const chat = chats.find((c) => c.id === chatId);

  if (!chat) return <Missing title="Chat" text="This chat no longer exists." />;
  const name = chat.contact.name;

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
        avatar={{ name, tint: tintFor(name) }}
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
      {error && (
        <p className={layout.error} role="alert">
          {error}
        </p>
      )}
      <Composer onSend={send} />
    </div>
  );
}
