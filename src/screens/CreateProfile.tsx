import { useMemo, useState, type FormEvent } from "react";
import { core } from "../core";
import { MAX_NAME } from "../core/types";
import { jpegDataUrl, photoBytes } from "../platform";
import { useNavigation } from "../navigation/Navigator";
import { Button } from "../ui/Button";
import { TextField } from "../ui/fields";
import { Icon } from "../ui/Icon";
import { ProfilePhoto } from "../ui/ProfilePhoto";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./CreateProfile.module.css";

/**
 * First launch: the user picks a name and the core creates the account. A photo chosen here is
 * uploaded once the account exists (#94). If that fails, it can be set again on Profile.
 */
export function CreateProfile() {
  const navigation = useNavigation();
  const [name, setName] = useState("");
  const [photo, setPhoto] = useState<Uint8Array | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const trimmed = name.trim();
  const preview = useMemo(() => (photo ? jpegDataUrl(photo) : undefined), [photo]);

  async function choosePhoto(file: File) {
    setError(null);
    try {
      setPhoto(await photoBytes(file));
    } catch {
      setError("This photo can't be used. Try another one.");
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!trimmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      await core.createAccount(trimmed);
      if (photo) {
        core.setProfilePhoto(photo).catch((e) => console.warn("The profile photo was not set:", e));
      }
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
        <ProfilePhoto
          name={trimmed}
          size={96}
          photo={preview}
          onChoose={choosePhoto}
          busy={busy}
        />
        <TextField
          label="Name"
          value={name}
          onChange={(event) => setName(event.target.value)}
          helper="Shown to the people you chat with. You can change it later."
          autoComplete="name"
          autoCapitalize="words"
          maxLength={MAX_NAME}
          autoFocus
          data-testid="create-profile-name"
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
        <Button type="submit" disabled={!trimmed || busy} data-testid="create-profile-continue">
          Continue
        </Button>
      </footer>
    </form>
  );
}
