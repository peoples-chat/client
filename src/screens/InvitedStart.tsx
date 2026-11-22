import { useEffect, useState, type FormEvent } from "react";
import { core } from "../core";
import { MAX_NAME, type Contact } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { Avatar } from "../ui/Avatar";
import { Button } from "../ui/Button";
import { TextField } from "../ui/fields";
import { Icon } from "../ui/Icon";
import { tintFor } from "../ui/people";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./InvitedStart.module.css";

/**
 * Someone without an account opened an invite link (#95): who invited them, and one step to an
 * account and the chat with that person. If the chat can't start after the account exists, the
 * button tries the chat again.
 */
export function InvitedStart({ link }: { link: string }) {
  const navigation = useNavigation();
  const { account } = useAppState();
  const [from, setFrom] = useState<Contact | null>(null);
  const [unusable, setUnusable] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const first = from?.name.split(/\s+/)[0];

  useEffect(() => {
    let active = true;
    core.previewInvite(link).then(
      (contact) => active && setFrom(contact),
      (e) => active && setUnusable(errorText(e)),
    );
    return () => {
      active = false;
    };
  }, [link]);

  async function start(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      if (!account) await core.createAccount(name);
      const chat = await core.addContact(link);
      navigation.reset([{ name: "chats" }, { name: "chat", chatId: chat.id }]);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  }

  if (unusable) {
    return (
      <div className={layout.screen}>
        <div className={layout.centered} role="alert">
          <p>This link can't be opened.</p>
          <p>{unusable}</p>
        </div>
        <footer className={styles.footer}>
          <Button onClick={() => navigation.reset([{ name: account ? "chats" : "welcome" }])}>Continue</Button>
        </footer>
      </div>
    );
  }

  return (
    <form className={layout.screen} onSubmit={start}>
      <div className={styles.top}>
        <Avatar name={from?.name ?? ""} size={84} tint={tintFor(from?.name ?? "")} photo={from?.photo} />
        <h1 className={styles.title}>{from ? `${from.name} invited you` : "You are invited"}</h1>
        <p className={styles.lead}>
          Pick a name and you are in a chat with {first ?? "them"}. No phone number or email needed.
        </p>
      </div>
      <main className={`${layout.scroll} ${styles.main}`}>
        {!account && (
          <TextField
            label="Your name"
            value={name}
            maxLength={MAX_NAME}
            autoComplete="name"
            autoCapitalize="words"
            onChange={(event) => setName(event.target.value)}
          />
        )}
        <div className={styles.keyNote}>
          <Icon name="key" size={22} className={styles.keyIcon} />
          <p>Your account key is created on this phone. Nobody else holds a copy.</p>
        </div>
        {error && (
          <p className={styles.error} role="alert">
            {error}
          </p>
        )}
      </main>
      <footer className={styles.footer}>
        <Button type="submit" disabled={busy || !from || (!account && !name.trim())}>
          {first ? `Start chatting with ${first}` : "Start chatting"}
        </Button>
        <Button variant="quiet" disabled={busy} onClick={() => navigation.reset([{ name: account ? "chats" : "welcome" }])}>
          This invitation is not for me
        </Button>
      </footer>
    </form>
  );
}
