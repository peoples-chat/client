import { useNavigation } from "../navigation/Navigator";
import { chatListItems, useAppState } from "../state";
import { IconButton } from "../ui/Button";
import { Chip, ChipRow } from "../ui/Chip";
import { SearchField } from "../ui/fields";
import { formatListTime } from "../ui/format";
import { tintFor } from "../ui/people";
import { ChatRow } from "../ui/rows";
import { TabHeader } from "../ui/ScreenHeader";
import { TabBar } from "../ui/TabBar";
import layout from "../ui/layout.module.css";
import styles from "./Chats.module.css";

/** The Chats tab: message requests and chats, newest first. */
export function Chats() {
  const state = useAppState();
  const navigation = useNavigation();
  const items = chatListItems(state);
  const now = Date.now();

  return (
    <div className={layout.screen}>
      <TabHeader
        title="Chats"
        accountName={state.account?.name ?? ""}
        accountPhoto={state.account?.photo}
        onOpenSettings={() => navigation.push({ name: "settings" })}
        action={<IconButton label="New chat" icon="edit" onClick={() => navigation.push({ name: "newChat" })} />}
      />
      {/* Search and the filters other than All are not built yet (#63). */}
      <SearchField label="Search" placeholder="Search" disabled />
      <ChipRow>
        <Chip selected>All</Chip>
        <Chip disabled>Unread</Chip>
        <Chip disabled>Groups</Chip>
      </ChipRow>
      <main className={layout.scroll}>
        {items.length === 0 ? (
          <div className={layout.centered}>
            <p>No chats yet.</p>
            <p>Start one with New chat.</p>
          </div>
        ) : (
          <ul className={styles.list}>
            {items.map((item) =>
              item.kind === "chat" ? (
                <li key={item.chat.id}>
                  <ChatRow
                    name={item.chat.contact.name}
                    tint={tintFor(item.chat.contact.name)}
                    photo={item.chat.contact.photo}
                    time={item.time ? formatListTime(item.time, now) : ""}
                    preview={item.chat.messages[item.chat.messages.length - 1]?.text ?? "No messages yet"}
                    onClick={() => navigation.push({ name: "chat", chatId: item.chat.id })}
                  />
                </li>
              ) : (
                <li key={item.invitation.id}>
                  <ChatRow
                    name={item.invitation.from.name}
                    tint={tintFor(item.invitation.from.name)}
                    photo={item.invitation.from.photo}
                    time={formatListTime(item.time, now)}
                    preview="Message request"
                    request
                    onClick={() => navigation.push({ name: "request", invitationId: item.invitation.id })}
                  />
                </li>
              ),
            )}
          </ul>
        )}
      </main>
      <TabBar />
    </div>
  );
}
