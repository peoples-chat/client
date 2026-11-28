import type { SentInvite } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { share } from "../platform";
import { chatListItems, useAppState } from "../state";
import { Button, IconButton } from "../ui/Button";
import { Icon } from "../ui/Icon";
import { Chip, ChipRow } from "../ui/Chip";
import { SearchField } from "../ui/fields";
import { formatListTime } from "../ui/format";
import { tintFor } from "../ui/people";
import { ChatRow } from "../ui/rows";
import { TabHeader } from "../ui/ScreenHeader";
import { TabBar } from "../ui/TabBar";
import layout from "../ui/layout.module.css";
import styles from "./Chats.module.css";

const DAY = 24 * 60 * 60 * 1000;
/** How many chats make the chat list stop offering invitations. */
const FIRST_DAYS_CHATS = 5;

/**
 * The Chats tab: message requests and chats, newest first. In the first days, a banner offers to
 * invite people, and the invitations still out follow the chats (#95).
 */
export function Chats() {
  const state = useAppState();
  const navigation = useNavigation();
  const items = chatListItems(state);
  const now = Date.now();
  const joinedChats = new Set(state.invites.flatMap((invite) => invite.joinedChatId ?? []));
  const pending = state.invites.filter((invite) => !invite.joinedChatId);
  // The first days: until there are five chats, or while invitations are still out.
  const showBanner = pending.length > 0 || state.chats.length < FIRST_DAYS_CHATS;

  return (
    <div className={layout.screen}>
      <TabHeader
        title="Chats"
        accountName={state.account?.name ?? ""}
        accountPhoto={state.account?.photo}
        onOpenSettings={() => navigation.push({ name: "settings" })}
        action={<IconButton label="New chat" icon="edit" data-testid="chats-new-chat" onClick={() => navigation.push({ name: "newChat" })} />}
      />
      {/* Search and the filters other than All are not built yet (#63). */}
      <SearchField label="Search" placeholder="Search" disabled />
      <ChipRow>
        <Chip selected>All</Chip>
        <Chip disabled>Unread</Chip>
        <Chip disabled>Groups</Chip>
      </ChipRow>
      {showBanner && (
        <section className={styles.banner} aria-labelledby="invite-banner">
          <div className={styles.bannerText}>
            <h2 id="invite-banner" className={styles.bannerTitle}>
              Bring your people
            </h2>
            <p className={styles.bannerLine}>
              {state.invites.length > 0
                ? `${joinedChats.size} of ${state.invites.length} ${joinedChats.size === 1 ? "has" : "have"} joined.`
                : "Invite the people you talk to."}
            </p>
          </div>
          <Button size="small" onClick={() => navigation.push({ name: "invite" })}>
            <Icon name="personAdd" size={20} strokeWidth={2.2} />
            Invite
          </Button>
        </section>
      )}
      <main className={layout.scroll}>
        {items.length === 0 && pending.length === 0 ? (
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
                    preview={
                      item.chat.messages[item.chat.messages.length - 1]?.text ??
                      (joinedChats.has(item.chat.id) ? "Joined from your invite. Say hi." : "No messages yet")
                    }
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
            {pending.map((invite) => (
              <li key={invite.id}>
                <PendingInvite invite={invite} now={now} />
              </li>
            ))}
          </ul>
        )}
      </main>
      <TabBar />
    </div>
  );
}

/** An invitation that is still out: who it is for, when it went, and a way to send it again. */
function PendingInvite({ invite, now }: { invite: SentInvite; now: number }) {
  const days = Math.floor((now - invite.createdAt) / DAY);
  const expired = now >= invite.expiresAt;
  const when = expired ? "The link has expired" : days === 0 ? "Invited today" : days === 1 ? "Invited yesterday" : `Invited ${days} days ago`;
  return (
    <div className={styles.pending}>
      <span className={styles.pendingAvatar} aria-hidden="true">
        {Array.from(invite.name)[0]?.toUpperCase()}
      </span>
      <span className={styles.pendingText}>
        <span className={styles.pendingName}>{invite.name}</span>
        <span className={styles.pendingWhen}>
          <Icon name="clock" size={15} />
          {when}
        </span>
      </span>
      {!expired && (
        <Button
          variant="quiet"
          size="small"
          aria-label={`Remind ${invite.name}`}
          onClick={() => share(`Here is the link again: ${invite.link}`, "Chat with me on peoples.chat").catch(() => {})}
        >
          Remind
        </Button>
      )}
    </div>
  );
}
