import { useState } from "react";
import { core } from "../core";
import { MIN_EXPORT_PASSWORD, type KeyCopy } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { saveAsFile } from "../platform";
import { Button } from "../ui/Button";
import { TextField } from "../ui/fields";
import { Icon, type IconName } from "../ui/Icon";
import { QrCode } from "../ui/QrCode";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./ExportKey.module.css";

type Kind = KeyCopy["kind"];

/** The exported key and how it is shown. It lives only in this screen's state. */
type Shown = { as: "qr"; key: string } | { as: "text"; key: string };

const FILE_NAME = "peoples.chat account key.txt";

/**
 * Export account key, reached from Account (#86). The design shows the protected copy; the
 * plain key is shown as text, as its option says. Every export asks for the device unlock first,
 * in the app shell.
 */
export function ExportKey() {
  const navigation = useNavigation();
  const [kind, setKind] = useState<Kind>("protected");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [shown, setShown] = useState<Shown | null>(null);
  const [copied, setCopied] = useState(false);
  const passwordReady = Array.from(password).length >= MIN_EXPORT_PASSWORD;

  async function exportKey(then: "file" | "qr" | "text") {
    setBusy(true);
    setError(null);
    try {
      const key = await core.exportAccountKey(kind === "protected" ? { kind, password } : { kind });
      if (then === "file") await saveAsFile(key, FILE_NAME);
      else setShown({ as: then, key });
    } catch (e) {
      // Closing the share sheet without saving is not an error.
      if (!(e instanceof DOMException && e.name === "AbortError")) setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  async function copy(key: string) {
    try {
      await navigator.clipboard.writeText(key);
      setCopied(true);
    } catch {
      setError("Could not copy the key.");
    }
  }

  if (shown) {
    return (
      <div className={layout.screen}>
        <ScreenHeader title="Export account key" onLeading={navigation.back} />
        <main className={`${layout.scroll} ${styles.main}`}>
          {shown.as === "qr" ? (
            <>
              <div className={styles.qr}>
                <QrCode value={shown.key} size={232} label="Your account key, protected with your password" />
              </div>
              <p className={styles.note}>Scan it with the other app. It asks for the password you chose.</p>
            </>
          ) : (
            <>
              <Warning />
              <p className={styles.key}>{shown.key}</p>
              <Button variant="secondary" onClick={() => copy(shown.key)} aria-live="polite">
                {copied ? "Copied" : "Copy"}
              </Button>
            </>
          )}
          {error && (
            <p className={layout.error} role="alert">
              {error}
            </p>
          )}
        </main>
        <footer className={styles.footer}>
          <Button onClick={navigation.back}>Done</Button>
        </footer>
      </div>
    );
  }

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Export account key" onLeading={navigation.back} />
      <main className={`${layout.scroll} ${styles.main}`}>
        <Warning />
        <div role="radiogroup" aria-label="Kind of copy" className={styles.kinds}>
          <KindOption
            kind="protected"
            selected={kind}
            onSelect={setKind}
            icon="lock"
            title="Protected with a password"
            description="Recommended. Opens in other Nostr apps."
          />
          <KindOption
            kind="plain"
            selected={kind}
            onSelect={setKind}
            icon="eye"
            title="Plain key"
            description="Shown as text, with no protection."
          />
        </div>
        {kind === "protected" && (
          <TextField
            label="Password for the copy"
            type="password"
            placeholder={`At least ${MIN_EXPORT_PASSWORD} characters`}
            helper="You need it to open the copy. Nobody can reset it."
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            autoComplete="new-password"
          />
        )}
        <p className={styles.note}>
          The key is your identity only. To bring your chat history as well, keep your recovery words.
        </p>
        {error && (
          <p className={layout.error} role="alert">
            {error}
          </p>
        )}
      </main>
      <footer className={styles.footer}>
        {kind === "protected" ? (
          <>
            <Button onClick={() => exportKey("file")} disabled={busy || !passwordReady}>
              <Icon name="download" size={20} strokeWidth={2.2} />
              Save as a file
            </Button>
            <Button variant="secondary" onClick={() => exportKey("qr")} disabled={busy || !passwordReady}>
              <Icon name="qr" size={20} strokeWidth={2.2} />
              Show as a QR code
            </Button>
          </>
        ) : (
          <Button onClick={() => exportKey("text")} disabled={busy}>
            <Icon name="eye" size={20} strokeWidth={2.2} />
            Show the key
          </Button>
        )}
      </footer>
    </div>
  );
}

function Warning() {
  return (
    <div className={styles.warning}>
      <Icon name="shield" size={22} className={styles.warningIcon} />
      <div className={styles.warningText}>
        <strong>Handle with care</strong>
        <span>Anyone who gets this key can act as you, and it can never be changed. Keep your copy offline.</span>
      </div>
    </div>
  );
}

interface KindOptionProps {
  kind: Kind;
  selected: Kind;
  onSelect: (kind: Kind) => void;
  icon: IconName;
  title: string;
  description: string;
}

function KindOption({ kind, selected, onSelect, icon, title, description }: KindOptionProps) {
  const checked = kind === selected;
  return (
    <button
      type="button"
      role="radio"
      aria-checked={checked}
      className={`${styles.kind} ${checked ? styles.checked : ""}`}
      onClick={() => onSelect(kind)}
    >
      <span className={styles.kindIcon} aria-hidden="true">
        <Icon name={icon} size={20} />
      </span>
      <span className={styles.kindText}>
        <span className={styles.kindTitle}>{title}</span>
        <span className={styles.kindDescription}>{description}</span>
      </span>
      <span className={styles.radio} aria-hidden="true">
        {checked && <Icon name="check" size={16} strokeWidth={2.6} />}
      </span>
    </button>
  );
}
