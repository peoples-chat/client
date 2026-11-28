import { useState, type FormEvent } from "react";
import { core } from "../core";
import { useNavigation } from "../navigation/Navigator";
import { canScanQrCodes } from "../platform";
import { useAppState } from "../state";
import { SearchField } from "../ui/fields";
import { tintFor } from "../ui/people";
import { ActionRow, ContactRow, SectionTitle } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./NewChat.module.css";

/**
 * Start a chat: paste an invite link, scan a QR code, or pick a contact. Groups and finding
 * people by name are in the design but not built yet, so they are disabled.
 */
export function NewChat({ link: opened }: { link?: string }) {
  const { chats } = useAppState();
  const navigation = useNavigation();
  const [link, setLink] = useState(opened ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const hasLink = link.trim().length > 0;
  const contacts = [...chats].sort((a, b) => a.contact.name.localeCompare(b.contact.name));

  async function startFromLink(event?: FormEvent) {
    event?.preventDefault();
    if (!hasLink || busy) return;
    setBusy(true);
    setError(null);
    try {
      const chat = await core.addContact(link);
      navigation.replace({ name: "chat", chatId: chat.id });
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  }

  return (
    <div className={layout.screen}>
      <ScreenHeader title="New chat" leading="close" onLeading={navigation.back} />
      <form onSubmit={startFromLink}>
        <SearchField
          label="Invite link"
          placeholder="Paste an invite link"
          value={link}
          onChange={(event) => {
            setLink(event.target.value);
            setError(null);
          }}
          enterKeyHint="go"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
      </form>
      {error && (
        <p className={layout.error} role="alert">
          {error}
        </p>
      )}
      <main className={layout.scroll}>
        {hasLink && (
          <ActionRow icon="link" label="Start a chat from this link" accent disabled={busy} onClick={() => startFromLink()} />
        )}
        <ActionRow icon="group" label="New group" accent disabled />
        <ActionRow
          icon="qr"
          label="Scan a QR code"
          accent
          disabled={!canScanQrCodes}
          testId="new-chat-scan"
          onClick={() => navigation.push({ name: "scan" })}
        />
        <ActionRow icon="link" label="Share my invite link" accent onClick={() => navigation.push({ name: "profile" })} />
        {contacts.length > 0 && (
          <>
            <SectionTitle>Contacts</SectionTitle>
            <ul className={styles.contacts} data-testid="new-chat-contacts">
              {contacts.map((chat) => (
                <li key={chat.id}>
                  <ContactRow
                    name={chat.contact.name}
                    tint={tintFor(chat.contact.name)}
                    photo={chat.contact.photo}
                    onClick={() => navigation.replace({ name: "chat", chatId: chat.id })}
                  />
                </li>
              ))}
            </ul>
          </>
        )}
      </main>
    </div>
  );
}
