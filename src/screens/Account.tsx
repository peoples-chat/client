import { useState } from "react";
import { core } from "../core";
import { useNavigation } from "../navigation/Navigator";
import { Button } from "../ui/Button";
import { ActionRow, InfoRow, SectionTitle } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./Account.module.css";

/**
 * Account, reached from Settings. Export account key (#86) and Delete account are built. Delete
 * asks once more before it deletes, because it cannot be undone. The design shows no
 * confirmation, so this one is minimal.
 */
export function Account() {
  const navigation = useNavigation();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function deleteAccount() {
    setBusy(true);
    setError(null);
    try {
      await core.deleteAccount();
      navigation.reset([{ name: "welcome" }]);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  }

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Account" bordered onLeading={navigation.back} />
      <main className={layout.scroll}>
        {/* Signer apps and recovery are not built yet (#63). The design's Username is left out
            for now: it would need a name service at peoples.chat (#93). */}
        <SectionTitle>Sign-in</SectionTitle>
        <InfoRow icon="key" label="Account key" description="Kept on this phone only" />
        <ActionRow
          icon="download"
          label="Export account key"
          description="Keep a copy of your own"
          onClick={() => navigation.push({ name: "exportKey" })}
        />
        <ActionRow icon="shield" label="Move the key to a signer app" description="For extra protection" disabled />
        <SectionTitle>Recovery</SectionTitle>
        <ActionRow icon="group" label="Guardians" disabled />
        <ActionRow icon="lock" label="Recovery passphrase" disabled />
        <SectionTitle>Leave</SectionTitle>
        {confirming ? (
          <section className={styles.confirm} aria-labelledby="delete-title">
            <h3 id="delete-title" className={styles.confirmTitle}>
              Delete your account?
            </h3>
            <p className={styles.confirmText}>
              Your profile is removed, you leave every chat, and your chats are deleted from this phone. This
              cannot be undone.
            </p>
            {error && (
              <p className={styles.error} role="alert">
                {error}
              </p>
            )}
            <div className={styles.actions}>
              <Button variant="secondary" onClick={() => setConfirming(false)} disabled={busy} autoFocus>
                Cancel
              </Button>
              <Button variant="dangerSolid" onClick={deleteAccount} disabled={busy} data-testid="account-delete-confirm">
                Delete account
              </Button>
            </div>
          </section>
        ) : (
          <ActionRow
            icon="trash"
            label="Delete account"
            description="Removes your profile and leaves every chat"
            danger
            chevron={false}
            onClick={() => setConfirming(true)}
            testId="account-delete"
          />
        )}
      </main>
    </div>
  );
}
