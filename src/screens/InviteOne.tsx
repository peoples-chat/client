import { useId, useState } from "react";
import { core } from "../core";
import { MAX_NAME, type SentInvite } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { canShare, share } from "../platform";
import { Button } from "../ui/Button";
import { TextField } from "../ui/fields";
import { Icon } from "../ui/Icon";
import { QrCode } from "../ui/QrCode";
import { ActionRow } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./InviteOne.module.css";

const MESSAGE = "Text me when the internet goes down. I am on peoples.chat, and this link puts you straight into a chat with me:";

/**
 * Invite one person (#95): a personal link, shared with a message or shown as a QR code. The
 * invitation is made when it is first shared or shown, so leaving the screen makes none. Asking
 * the person to be a guardian waits for recovery.
 */
export function InviteOne() {
  const navigation = useNavigation();
  const messageId = useId();
  const [name, setName] = useState("");
  const [message, setMessage] = useState(MESSAGE);
  const [invite, setInvite] = useState<SentInvite | null>(null);
  const [showQr, setShowQr] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const who = name.trim();

  async function theInvite(): Promise<SentInvite> {
    if (invite) return invite;
    const made = await core.createInvite(who);
    setInvite(made);
    return made;
  }

  async function shareLink() {
    setBusy(true);
    setError(null);
    try {
      const made = await theInvite();
      await share(`${message.trim()} ${made.link}`, "Chat with me on peoples.chat");
      navigation.reset([{ name: "chats" }]);
    } catch (e) {
      // Closing the web share sheet rejects; that isn't an error to show.
      if (!(e instanceof DOMException && e.name === "AbortError")) setError(errorText(e));
      setBusy(false);
    }
  }

  async function showCode() {
    setBusy(true);
    setError(null);
    try {
      await theInvite();
      setShowQr(true);
    } catch (e) {
      setError(errorText(e));
    }
    setBusy(false);
  }

  if (showQr && invite) {
    return (
      <div className={layout.screen}>
        <ScreenHeader title="Invite one person" onLeading={() => setShowQr(false)} />
        <main className={`${layout.scroll} ${styles.main}`}>
          <p className={styles.note}>{invite.name} scans this code with their phone's camera to get the link.</p>
          <div className={styles.qr}>
            <QrCode value={invite.link} size={232} label={`Invitation for ${invite.name}`} />
          </div>
          <p className={styles.note}>The link works once and stops working after 7 days.</p>
        </main>
        <footer className={styles.footer}>
          <Button onClick={() => navigation.reset([{ name: "chats" }])}>Done</Button>
        </footer>
      </div>
    );
  }

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Invite one person" onLeading={navigation.back} />
      <main className={`${layout.scroll} ${styles.main}`}>
        <TextField
          label="Who is it for?"
          value={name}
          maxLength={MAX_NAME}
          autoCapitalize="words"
          helper="Only you see this name. It tells you who has joined."
          onChange={(event) => setName(event.target.value)}
          disabled={invite !== null}
        />
        <div className={styles.field}>
          <label htmlFor={messageId} className={styles.label}>
            Your message
          </label>
          <textarea
            id={messageId}
            className={styles.textarea}
            rows={4}
            value={message}
            onChange={(event) => setMessage(event.target.value)}
          />
          <span className={styles.helper}>The link goes at the end.</span>
        </div>
        {/* Guardians come with recovery. */}
        <ActionRow
          icon="shield"
          label={`Ask ${who || "them"} to be a guardian`}
          description="One of five people who can help you get your account back"
          chevron={false}
          disabled
        />
        <p className={styles.note}>The link works once and stops working after 7 days.</p>
        {error && (
          <p className={styles.error} role="alert">
            {error}
          </p>
        )}
      </main>
      <footer className={styles.footer}>
        <Button onClick={shareLink} disabled={!who || busy || !canShare}>
          <Icon name="share" size={20} />
          Share the link
        </Button>
        <Button variant="secondary" onClick={showCode} disabled={!who || busy}>
          <Icon name="qr" size={20} />
          Show as a QR code
        </Button>
      </footer>
    </div>
  );
}
