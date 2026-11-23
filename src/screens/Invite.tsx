import { useNavigation } from "../navigation/Navigator";
import { Button } from "../ui/Button";
import { Icon, type IconName } from "../ui/Icon";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import styles from "./Invite.module.css";

/**
 * Bring your people (#95): the ways to invite someone. A whole group waits for group chats, so
 * that option is disabled.
 */
export function Invite() {
  const navigation = useNavigation();
  return (
    <div className={layout.screen}>
      <ScreenHeader title="Bring your people" onLeading={navigation.back} />
      <main className={`${layout.scroll} ${styles.main}`}>
        <p className={styles.intro}>
          A messenger is only as good as the people on it. Invite a few now. Each one lands in a chat with you the
          moment they sign up.
        </p>
        <Option
          icon="personAdd"
          title="Invite one person"
          description="A personal link that works once."
          onClick={() => navigation.push({ name: "inviteOne" })}
        />
        <Option icon="group" title="Bring a whole group" description="One link for the family, the team or the neighbours." />
        <Option
          icon="qr"
          title="Meet in person"
          description="They scan your code and you are connected."
          onClick={() => navigation.push({ name: "profile" })}
        />
        <div className={styles.note}>
          <Icon name="lock" size={22} className={styles.noteIcon} />
          <p>Your address book is never uploaded. You choose who gets a link.</p>
        </div>
      </main>
      <footer className={styles.footer}>
        <Button variant="quiet" onClick={navigation.back}>
          I will do this later
        </Button>
      </footer>
    </div>
  );
}

interface OptionProps {
  icon: IconName;
  title: string;
  description: string;
  /** Without it, the option is not built yet and is disabled. */
  onClick?: () => void;
}

function Option({ icon, title, description, onClick }: OptionProps) {
  return (
    <button type="button" className={styles.option} onClick={onClick} disabled={!onClick}>
      <span className={styles.optionIcon}>
        <Icon name={icon} size={24} />
      </span>
      <span className={styles.optionText}>
        <span className={styles.optionTitle}>{title}</span>
        <span className={styles.optionDescription}>{description}</span>
      </span>
      <Icon name="chevron" size={18} className={styles.chevron} />
    </button>
  );
}
