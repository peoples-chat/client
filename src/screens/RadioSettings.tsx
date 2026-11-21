import { useNavigation } from "../navigation/Navigator";
import { Icon } from "../ui/Icon";
import { ActionRow } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import styles from "./RadioSettings.module.css";

/**
 * Radio, reached from Settings. A placeholder until pairing lands with #24: the design's
 * contacts-by-mesh summary, heard radios and mesh channels need a paired radio first.
 */
export function RadioSettings() {
  const navigation = useNavigation();

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
              No radio paired
            </h2>
            <p className={styles.description}>Pair a MeshCore radio to keep chatting when the internet is down.</p>
          </div>
        </section>
        {/* Pairing is not built yet (#24). */}
        <ActionRow icon="plus" label="Pair a radio" accent chevron={false} disabled />
      </main>
    </div>
  );
}
