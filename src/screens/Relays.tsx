import type { Relay, RelayOperator, RelayStatus } from "../core/types";
import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { ActionRow, InfoRow, SectionTitle } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import styles from "./Relays.module.css";

/**
 * Relays and servers, reached from Settings → Privacy (#88): the account's relays as the core
 * reports them, with who runs each and whether it can be reached. Adding a relay, and the media
 * and notification servers, are not built yet, so the intro doesn't offer to swap relays.
 */
export function Relays() {
  const navigation = useNavigation();
  const { relays } = useAppState();

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Relays and servers" bordered onLeading={navigation.back} />
      <main className={layout.scroll}>
        <p className={styles.intro}>{intro(relays.length)}</p>
        <SectionTitle>Relays</SectionTitle>
        {relays.map((relay) => (
          <InfoRow
            key={relay.url}
            icon="globe"
            label={relayName(relay.url)}
            description={<Description relay={relay} />}
          />
        ))}
        <ActionRow icon="plus" label="Add a relay" accent chevron={false} disabled />
        <SectionTitle>Other servers</SectionTitle>
        <ActionRow icon="image" label="Media" description="Not used yet" disabled />
        <ActionRow icon="bell" label="Notifications" description="Not used yet" disabled />
      </main>
    </div>
  );
}

const OPERATORS: Record<RelayOperator, string> = {
  peoplesChat: "Run by peoples.chat",
  independent: "Independent",
};

const STATUSES: Record<RelayStatus, string> = {
  connected: "Connected",
  connecting: "Connecting",
  unreachable: "Not reachable",
};

/** "Independent. Connected", in red when the relay can't be reached. */
function Description({ relay }: { relay: Relay }) {
  const text = `${OPERATORS[relay.operator]}. ${STATUSES[relay.status]}`;
  return relay.status === "unreachable" ? <span className={styles.unreachable}>{text}</span> : text;
}

const COUNTS = ["", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];

/** The count in words, as the design writes it. */
function intro(count: number): string {
  if (count === 1) return "Your messages travel through one relay. It can't read your messages.";
  const relays = count === 0 ? "relays" : `${COUNTS[count] ?? count} relays`;
  return `Your messages travel through ${relays}. None of them can read your messages.`;
}

/** A relay's address without `wss://`, as the design shows it. */
function relayName(url: string): string {
  return url.replace(/^wss?:\/\//i, "").replace(/\/$/, "");
}
