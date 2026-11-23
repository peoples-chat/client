import { useCallback, useEffect, useState } from "react";
import { core } from "../core";
import type { RadioPort } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { ActionRow, InfoRow, SectionTitle } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./PairRadio.module.css";

/**
 * Pair a radio, reached from Radio (#24): the USB ports a radio may be plugged into, as the core
 * lists them. Choosing one pairs the radio there, and Radio then shows it. The design has no
 * pairing screen, so this follows the settings screens. Phones will pair over Bluetooth (#23).
 */
export function PairRadio() {
  const navigation = useNavigation();
  const { radio } = useAppState();
  const [ports, setPorts] = useState<RadioPort[] | null>(null);
  const [pairing, setPairing] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const look = useCallback(async () => {
    setError(null);
    try {
      setPorts(await core.radioPorts());
    } catch (e) {
      setPorts([]);
      setError(errorText(e));
    }
  }, []);

  useEffect(() => {
    void look();
  }, [look]);

  async function pair(path: string) {
    setPairing(path);
    setError(null);
    try {
      await core.pairRadio(path);
      navigation.back();
    } catch (e) {
      setError(errorText(e));
      setPairing(null);
    }
  }

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Pair a radio" bordered onLeading={navigation.back} />
      <main className={layout.scroll}>
        <p className={styles.intro}>
          Plug your MeshCore radio into this computer with a USB cable, then choose its port. It needs MeshCore's
          companion firmware for USB. The app only reads the radio's settings, and never changes them.
        </p>
        {radio.paired && <p className={styles.intro}>The radio you choose takes the place of {radio.paired.name}.</p>}
        <SectionTitle>USB ports</SectionTitle>
        {ports === null ? (
          <InfoRow icon="clock" label="Looking for ports" />
        ) : ports.length === 0 ? (
          <InfoRow icon="info" label="Nothing plugged in" description="Plug the radio in, then look again." />
        ) : (
          ports.map((port) => (
            <ActionRow
              key={port.path}
              icon="radio"
              label={port.description ?? port.path}
              description={pairing === port.path ? "Pairing" : port.description ? port.path : undefined}
              chevron={false}
              disabled={pairing !== null}
              onClick={() => void pair(port.path)}
            />
          ))
        )}
        <ActionRow
          icon="search"
          label="Look again"
          chevron={false}
          disabled={pairing !== null}
          onClick={() => void look()}
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
