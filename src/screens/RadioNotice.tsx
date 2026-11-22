import { useNavigation } from "../navigation/Navigator";
import { Icon, type IconName } from "../ui/Icon";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import styles from "./RadioNotice.module.css";

/** What radio protects and what it doesn't, in plain words (spec: Security and privacy). */
const POINTS: { icon: IconName; title: string; text: string }[] = [
  {
    icon: "lock",
    title: "Only the chat can read them",
    text: "Radio messages are end-to-end encrypted, like the rest of the chat.",
  },
  {
    icon: "person",
    title: "Sent by someone in the chat",
    text: "Radio proves only that a message comes from someone in the chat. Over the internet it also proves which device sent it.",
  },
  {
    icon: "key",
    title: "The same keys until you are online",
    text: "Radio keeps using the keys from your last time online. Someone who steals them later can read every radio message sent with them.",
  },
  {
    icon: "radio",
    title: "Seen by anyone in range",
    text: "Anyone nearby with a radio can see that yours sends, and when, and can find roughly where it is.",
  },
  {
    icon: "info",
    title: "Delivery is not guaranteed",
    text: "A radio message can get lost. A check mark means the other radio confirmed it.",
  },
];

/**
 * Radio and privacy, opened from a chat's notice while messages go by radio or wait (#48). The
 * spec asks the app to say plainly what radio does not protect.
 */
export function RadioNotice() {
  const navigation = useNavigation();

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Radio and privacy" bordered onLeading={navigation.back} />
      <main className={layout.scroll}>
        <p className={styles.intro}>
          When the internet is down, messages go by radio if your contact's radio can be heard. Radio protects less than
          the internet:
        </p>
        <ul className={styles.points}>
          {POINTS.map((point) => (
            <li key={point.title} className={styles.point}>
              <Icon name={point.icon} size={22} className={styles.icon} />
              <div>
                <h2 className={styles.title}>{point.title}</h2>
                <p className={styles.text}>{point.text}</p>
              </div>
            </li>
          ))}
        </ul>
      </main>
    </div>
  );
}
