import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { core } from "../core";
import { MAX_ABOUT, MAX_NAME, type Account } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { canShare, photoBytes, share, UnreadablePhoto } from "../platform";
import { useAppState } from "../state";
import { Button } from "../ui/Button";
import { TextField } from "../ui/fields";
import { ProfilePhoto } from "../ui/ProfilePhoto";
import { QrCode } from "../ui/QrCode";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./Profile.module.css";

/**
 * The user's profile and the code and link others use to start a chat. Name and About save when
 * the user leaves a field or presses Enter (#93). The design's Username is left out for now: it
 * would need a name service at peoples.chat.
 */
export function Profile() {
  const { account } = useAppState();
  const navigation = useNavigation();
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle");
  const resetTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(resetTimer.current), []);

  if (!account) return null;
  // The QR code keeps the nostr: link, which White Noise scans too; sharing uses the web link,
  // which opens the app when tapped (#72).
  const link = account.shareLink;

  async function copyLink() {
    try {
      await navigator.clipboard.writeText(link);
      setCopy("copied");
    } catch {
      setCopy("failed");
    }
    clearTimeout(resetTimer.current);
    resetTimer.current = setTimeout(() => setCopy("idle"), 2000);
  }

  async function shareLink() {
    try {
      await share(link, "Chat with me on peoples.chat");
    } catch {
      // Closing the web share sheet rejects; nothing to do.
    }
  }

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Profile" onLeading={navigation.back} />
      <main className={`${layout.scroll} ${styles.main}`}>
        <PhotoPicker account={account} />
        <ProfileFields account={account} />
        <section className={styles.code} aria-labelledby="code-title">
          <div className={styles.qrTile}>
            <QrCode value={account.inviteLink} size={84} label="Your QR code" />
          </div>
          <div className={styles.codeText}>
            <h2 id="code-title" className={styles.codeTitle}>
              Your code and link
            </h2>
            <p className={styles.codeDescription}>People scan it or open the link to start a chat.</p>
            <div className={styles.buttons}>
              <Button size="small" onClick={shareLink} disabled={!canShare}>
                Share
              </Button>
              <Button size="small" variant="secondary" onClick={copyLink} aria-live="polite">
                {copy === "copied" ? "Copied" : copy === "failed" ? "Could not copy" : "Copy"}
              </Button>
            </div>
          </div>
        </section>
        <PublicKeySection npub={account.npub} />
      </main>
    </div>
  );
}

/**
 * The user's public key as an npub, with Copy (#129). White Noise takes an npub, and its Android
 * app neither reads an nprofile nor opens peoples.chat links.
 */
function PublicKeySection({ npub }: { npub: string }) {
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle");
  const resetTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(resetTimer.current), []);

  async function copyNpub() {
    setCopy(await copyPublicKey(npub, navigator.clipboard));
    clearTimeout(resetTimer.current);
    resetTimer.current = setTimeout(() => setCopy("idle"), 2000);
  }

  return (
    <section className={styles.key} aria-labelledby="key-title" data-testid="profile-npub-section">
      <h2 id="key-title" className={styles.codeTitle}>
        Your public key
      </h2>
      <p className={styles.codeDescription}>For people who add you from White Noise or another Marmot app.</p>
      <p className={styles.npub} data-testid="profile-npub">
        {npub}
      </p>
      <div className={styles.buttons}>
        <Button
          size="small"
          variant="secondary"
          onClick={copyNpub}
          aria-live="polite"
          data-testid="profile-npub-copy"
        >
          {copy === "copied" ? "Copied" : copy === "failed" ? "Could not copy" : "Copy"}
        </Button>
      </div>
    </section>
  );
}

/** Hands the plain npub, and nothing else, to the clipboard (#129). */
export async function copyPublicKey(
  npub: string,
  clipboard: Pick<Clipboard, "writeText">,
): Promise<"copied" | "failed"> {
  try {
    await clipboard.writeText(npub);
    return "copied";
  } catch {
    return "failed";
  }
}

type PhotoState = { kind: "idle" } | { kind: "uploading" } | { kind: "removing" } | { kind: "failed"; message: string };

/** The photo, with the button that sets a new one, and a way to remove it (#94). */
function PhotoPicker({ account }: { account: Account }) {
  const [state, setState] = useState<PhotoState>({ kind: "idle" });
  const busy = state.kind === "uploading" || state.kind === "removing";

  async function choose(file: File) {
    setState({ kind: "uploading" });
    try {
      await core.setProfilePhoto(await photoBytes(file));
      setState({ kind: "idle" });
    } catch (error) {
      const message =
        error instanceof UnreadablePhoto ? "This photo can't be used. Try another one." : errorText(error);
      setState({ kind: "failed", message });
    }
  }

  async function remove() {
    setState({ kind: "removing" });
    try {
      await core.removeProfilePhoto();
      setState({ kind: "idle" });
    } catch (error) {
      setState({ kind: "failed", message: errorText(error) });
    }
  }

  return (
    <div className={styles.photo}>
      <ProfilePhoto name={account.name} size={88} photo={account.photo} onChoose={choose} busy={busy} />
      {state.kind === "failed" ? (
        <p className={styles.saveError} role="alert">
          {state.message}
        </p>
      ) : busy ? (
        <p className={styles.saveState} aria-live="polite">
          {state.kind === "uploading" ? "Uploading the photo…" : "Removing the photo…"}
        </p>
      ) : (
        account.photo && (
          <button type="button" className={styles.removePhoto} onClick={remove}>
            Remove photo
          </button>
        )
      )}
    </div>
  );
}

type SaveState = { kind: "idle" } | { kind: "saving" } | { kind: "saved" } | { kind: "failed"; message: string };

/** Name and About, saved through the core when they change. */
function ProfileFields({ account }: { account: Account }) {
  const [name, setName] = useState(account.name);
  const [about, setAbout] = useState(account.about);
  const [save, setSave] = useState<SaveState>({ kind: "idle" });
  const editing = useRef(false);

  // A change from elsewhere shows, unless the user is typing.
  useEffect(() => {
    if (editing.current) return;
    setName(account.name);
    setAbout(account.about);
  }, [account.name, account.about]);

  async function saveChanges() {
    editing.current = false;
    if (name.trim() === account.name && about.trim() === account.about) {
      setName(account.name);
      setAbout(account.about);
      return;
    }
    setSave({ kind: "saving" });
    try {
      const saved = await core.updateProfile(name, about);
      setName(saved.name);
      setAbout(saved.about);
      setSave({ kind: "saved" });
    } catch (error) {
      setSave({ kind: "failed", message: errorText(error) });
    }
  }

  function leaveOnEnter(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") event.currentTarget.blur();
  }

  return (
    <>
      <TextField
        label="Name"
        value={name}
        maxLength={MAX_NAME}
        autoComplete="name"
        onFocus={() => (editing.current = true)}
        onChange={(event) => setName(event.target.value)}
        onBlur={saveChanges}
        onKeyDown={leaveOnEnter}
      />
      <TextField
        label="About"
        value={about}
        maxLength={MAX_ABOUT}
        placeholder="A few words about you"
        onFocus={() => (editing.current = true)}
        onChange={(event) => setAbout(event.target.value)}
        onBlur={saveChanges}
        onKeyDown={leaveOnEnter}
      />
      {save.kind === "failed" ? (
        <p className={styles.saveError} role="alert">
          {save.message}
        </p>
      ) : (
        <p className={styles.saveState} aria-live="polite">
          {save.kind === "saving" ? "Saving…" : save.kind === "saved" ? "Saved" : ""}
        </p>
      )}
    </>
  );
}
