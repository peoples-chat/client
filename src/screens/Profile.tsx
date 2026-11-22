import { useEffect, useRef, useState } from "react";
import { useNavigation } from "../navigation/Navigator";
import { canShare, share } from "../platform";
import { useAppState } from "../state";
import { Button } from "../ui/Button";
import { TextField } from "../ui/fields";
import { ProfilePhoto } from "../ui/ProfilePhoto";
import { QrCode } from "../ui/QrCode";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import styles from "./Profile.module.css";

/**
 * The user's profile and the code and link others use to start a chat. Editing the profile is
 * not built yet, so the fields are disabled.
 */
export function Profile() {
  const { account } = useAppState();
  const navigation = useNavigation();
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle");
  const resetTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(resetTimer.current), []);

  if (!account) return null;
  const link = account.inviteLink;

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
        <ProfilePhoto name={account.name} size={88} />
        <TextField label="Name" value={account.name} disabled />
        <TextField label="About" value="" placeholder="A few words about you" disabled />
        <TextField label="Username" value="" placeholder="name@domain" helper="People can find you by this name." disabled />
        <section className={styles.code} aria-labelledby="code-title">
          <div className={styles.qrTile}>
            <QrCode value={link} size={84} label="Your QR code" />
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
      </main>
    </div>
  );
}
