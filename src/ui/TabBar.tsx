import { Icon, type IconName } from "./Icon";
import styles from "./TabBar.module.css";

/**
 * The main tabs: Chats and Calls. Calls is not built yet, so it is disabled and Chats is the
 * only tab for now.
 */
export function TabBar() {
  return (
    <nav aria-label="Main" className={styles.bar}>
      <TabButton icon="chat" label="Chats" active />
      <TabButton icon="phone" label="Calls" disabled />
    </nav>
  );
}

interface TabButtonProps {
  icon: IconName;
  label: string;
  active?: boolean;
  disabled?: boolean;
  onClick?: () => void;
}

function TabButton({ icon, label, active, disabled, onClick }: TabButtonProps) {
  return (
    <button
      type="button"
      className={`${styles.tab} ${active ? styles.active : ""}`}
      aria-current={active ? "page" : undefined}
      disabled={disabled}
      onClick={onClick}
    >
      <span className={styles.pill}>
        <Icon name={icon} />
      </span>
      <span className={styles.label}>{label}</span>
    </button>
  );
}
