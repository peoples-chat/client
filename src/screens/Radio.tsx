import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { Button, IconButton } from "../ui/Button";
import { Icon } from "../ui/Icon";
import { TabHeader } from "../ui/ScreenHeader";
import { TabBar } from "../ui/TabBar";
import layout from "../ui/layout.module.css";
import styles from "./Radio.module.css";

/** The Radio tab. A placeholder until pairing lands with #24. */
export function Radio() {
  const { account } = useAppState();
  const navigation = useNavigation();

  return (
    <div className={layout.screen}>
      <TabHeader
        title="Radio"
        accountName={account?.name ?? ""}
        onOpenSettings={() => navigation.push({ name: "settings" })}
        action={<IconButton label="Pair another radio" icon="plus" disabled />}
      />
      <main className={layout.scroll}>
        <section className={styles.card} aria-labelledby="radio-status">
          <div className={styles.status}>
            <div className={styles.radioIcon} aria-hidden="true">
              <Icon name="radio" size={22} />
            </div>
            <div className={styles.text}>
              <h2 id="radio-status" className={styles.title}>
                No radio paired
              </h2>
              <p className={styles.description}>Pair a MeshCore radio to keep chatting when the internet is down.</p>
            </div>
          </div>
          <Button disabled>Pair a radio</Button>
        </section>
      </main>
      <TabBar active="radio" onSelect={(tab) => tab !== "radio" && navigation.reset([{ name: tab }])} />
    </div>
  );
}
