import { useNavigation } from "../navigation/Navigator";
import { ActionRow, SectionTitle } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";

/**
 * Privacy, reached from Settings. Only Relays and servers is built (#88). The other settings are
 * disabled until they are, and show no on or off state, since they have none yet.
 */
export function Privacy() {
  const navigation = useNavigation();

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Privacy" bordered onLeading={navigation.back} />
      <main className={layout.scroll}>
        <ActionRow icon="block" label="Blocked" disabled />
        <SectionTitle>Messaging</SectionTitle>
        <ActionRow icon="checks" label="Read receipts" chevron={false} disabled />
        <ActionRow icon="chat" label="Typing indicators" chevron={false} disabled />
        <ActionRow icon="timer" label="Default timer for new chats" disabled />
        <SectionTitle>App security</SectionTitle>
        <ActionRow
          icon="lock"
          label="App lock"
          description="Ask for Face ID or passcode to open"
          chevron={false}
          disabled
        />
        <ActionRow icon="eye" label="Hide screen in the app switcher" chevron={false} disabled />
        <SectionTitle>Radio</SectionTitle>
        <ActionRow
          icon="radio"
          label="Share my radio address"
          description="With direct contacts only"
          chevron={false}
          disabled
        />
        <SectionTitle>Advanced</SectionTitle>
        <ActionRow
          icon="globe"
          label="Relays and servers"
          onClick={() => navigation.push({ name: "relays" })}
          testId="privacy-relays"
        />
      </main>
    </div>
  );
}
