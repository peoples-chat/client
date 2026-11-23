import { useState } from "react";
import { core } from "../core";
import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { Icon } from "../ui/Icon";
import { ActionRow, SectionTitle, SwitchRow } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./RadioSettings.module.css";

/**
 * Radio, reached from Settings: the radio the core uses, and the switch that sends everything by
 * radio, for testing until the app chooses by itself (#40). Pairing comes with #24; the design's
 * contacts-by-mesh summary, heard radios and mesh channels need a paired radio first.
 */
export function RadioSettings() {
  const navigation = useNavigation();
  const { radio } = useAppState();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

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

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Radio" bordered onLeading={navigation.back} />
      <main className={layout.scroll}>
        <section className={styles.card} aria-labelledby="radio-status">
          <div className={styles.radioIcon} aria-hidden="true">
            <Icon name="radio" size={22} />
          </div>
          <div className={styles.text}>
            <h2 id="radio-status" className={styles.title}>
              {radio.paired ? radio.paired.name : "No radio paired"}
            </h2>
            <p className={styles.description}>
              {radio.paired
                ? radio.paired.connected
                  ? "Connected"
                  : "Not connected"
                : "Pair a MeshCore radio to keep chatting when the internet is down."}
            </p>
          </div>
        </section>
        {/* Pairing is not built yet (#24). */}
        <ActionRow icon="plus" label="Pair a radio" accent chevron={false} disabled />
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
