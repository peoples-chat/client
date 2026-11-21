import { useState, type FormEvent } from "react";
import { core } from "../core";
import { useNavigation } from "../navigation/Navigator";
import { Button } from "../ui/Button";
import { TextField } from "../ui/fields";
import { Icon } from "../ui/Icon";
import { ProfilePhoto } from "../ui/ProfilePhoto";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./CreateProfile.module.css";

/** First launch: the user picks a name and the core creates the account. */
export function CreateProfile() {
  const navigation = useNavigation();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const trimmed = name.trim();

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!trimmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      await core.createAccount(trimmed);
      navigation.reset([{ name: "chats" }]);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  }

  return (
    <form className={layout.screen} onSubmit={submit}>
      <ScreenHeader title="Your profile" onLeading={navigation.back} />
      <main className={`${layout.scroll} ${styles.main}`}>
        <ProfilePhoto name={trimmed} size={96} />
        <TextField
          label="Name"
          value={name}
          onChange={(event) => setName(event.target.value)}
          helper="Shown to the people you chat with. You can change it later."
          autoComplete="name"
          autoCapitalize="words"
          maxLength={64}
          autoFocus
        />
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
        <Button type="submit" disabled={!trimmed || busy}>
          Continue
        </Button>
      </footer>
    </form>
  );
}
