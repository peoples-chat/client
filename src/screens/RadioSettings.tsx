import { useState } from "react";
import { core } from "../core";
import { presetText } from "../core/radio";
import type { PairedRadio } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { Button } from "../ui/Button";
import { Icon } from "../ui/Icon";
import { ActionRow, SectionTitle, SwitchRow } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./RadioSettings.module.css";

/**
 * Radio, reached from Settings: the paired radio with its connection and preset (#24), how many
 * contacts have a radio on each mesh (#34), and the switch that sends everything by radio, for
 * testing (#40). Computers pair over USB; phones will pair over Bluetooth (#23). The design's
 * heard radios (#44) and mesh channels come later.
 */
export function RadioSettings() {
  const navigation = useNavigation();
  const { radio, chats } = useAppState();
  const onMeshCore = chats.filter((chat) => chat.contact.meshes?.includes("meshCore")).length;
  const [busy, setBusy] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const paired = radio.paired;

  async function force(forced: boolean) {
    setBusy(true);
    setError(null);
    try {
      await core.setForceRadio(forced);
    } catch (e) {
      setError(errorText(e));
    }
    setBusy(false);
  }

  async function forget() {
    setBusy(true);
    setError(null);
    try {
      await core.forgetRadio();
      setForgetting(false);
    } catch (e) {
      setError(errorText(e));
    }
    setBusy(false);
  }

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Radio" bordered onLeading={navigation.back} />
      <main className={layout.scroll}>
        <section className={styles.card} aria-labelledby="radio-status">
          <div className={styles.cardTop}>
            <div className={styles.radioIcon} aria-hidden="true">
              <Icon name="radio" size={22} />
            </div>
            <div className={styles.text}>
              <h2 id="radio-status" className={styles.title}>
                {paired ? paired.name : "No radio paired"}
              </h2>
              <p className={styles.description}>
                {paired ? connectionText(paired) : "Pair a MeshCore radio to keep chatting when the internet is down."}
              </p>
            </div>
          </div>
          {paired && (
            <>
              <div className={styles.detail}>
                <span>Preset</span>
                <span className={styles.value}>{presetText(paired.preset)}</span>
              </div>
              {paired.port && (
                <div className={styles.detail}>
                  <span>Port</span>
                  <span className={styles.value}>{paired.port}</span>
                </div>
              )}
              {!paired.preset.expected && (
                <p className={styles.warning} role="note">
                  This isn't the EU/UK (Narrow) preset that the app expects: 869.618 MHz, 62.5 kHz, spreading factor 8.
                  The radio reaches only radios with its own settings. The app never changes them; the MeshCore app can.
                </p>
              )}
            </>
          )}
        </section>
        <ActionRow
          icon="plus"
          label={paired ? "Pair another radio" : "Pair a radio"}
          description={radio.canPair ? undefined : "Over USB on a computer, for now"}
          accent
          chevron={radio.canPair}
          disabled={!radio.canPair || busy}
          onClick={() => navigation.push({ name: "pairRadio" })}
        />
        {paired &&
          (forgetting ? (
            <section className={styles.confirm} aria-labelledby="forget-title">
              <h3 id="forget-title" className={styles.confirmTitle}>
                Forget {paired.name}?
              </h3>
              <p className={styles.confirmText}>
                The app stops using this radio and won't connect to it again. Nothing on the radio changes.
              </p>
              <div className={styles.actions}>
                <Button variant="secondary" onClick={() => setForgetting(false)} disabled={busy} autoFocus>
                  Cancel
                </Button>
                <Button variant="dangerSolid" onClick={forget} disabled={busy}>
                  Forget
                </Button>
              </div>
            </section>
          ) : (
            <ActionRow
              icon="trash"
              label="Forget this radio"
              danger
              chevron={false}
              disabled={busy}
              onClick={() => setForgetting(true)}
            />
          ))}
        <SectionTitle>Your contacts by mesh</SectionTitle>
        <MeshRow name="MeshCore" count={onMeshCore} of={chats.length} />
        <SectionTitle>Testing</SectionTitle>
        <SwitchRow
          icon="radio"
          label="Send everything by radio"
          description="Also while the internet works. Until the app chooses by itself."
          checked={radio.forced}
          disabled={busy}
          onChange={force}
        />
        {error && (
          <p className={layout.error} role="alert">
            {error}
          </p>
        )}
      </main>
    </div>
  );
}

/** How the radio is connected, as the design says it: "Connected by Bluetooth". */
function connectionText(radio: PairedRadio): string {
  switch (radio.connection) {
    case "connected":
      return radio.port ? "Connected by USB" : "Connected";
    case "connecting":
      return "Looking for it. Check that it's plugged in.";
    case "disconnected":
      return "Not connected";
  }
}

/** How many of `of` contacts have a radio on the mesh, as a bar and a number. */
function MeshRow({ name, count, of }: { name: string; count: number; of: number }) {
  return (
    <div className={styles.meshRow}>
      <span className={styles.meshName}>{name}</span>
      <span className={styles.meshBar} aria-hidden="true">
        <span style={{ width: `${of === 0 ? 0 : (100 * count) / of}%` }} />
      </span>
      <span className={styles.meshCount} aria-label={`${count} of ${of} contacts`}>
        {count}
      </span>
    </div>
  );
}
