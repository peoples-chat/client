import { Icon, type IconName } from "./Icon";
import styles from "./TabBar.module.css";

export type Tab = "chats" | "radio";

interface TabBarProps {
  active: Tab;
  onSelect: (tab: Tab) => void;
}

/** The main tabs. Calls is in the design but not built yet, so it is disabled. */
export function TabBar({ active, onSelect }: TabBarProps) {
  return (
    <nav aria-label="Main" className={styles.bar}>
      <TabButton icon="chat" label="Chats" active={active === "chats"} onClick={() => onSelect("chats")} />
      <TabButton icon="phone" label="Calls" disabled />
      <TabButton icon="radio" label="Radio" active={active === "radio"} onClick={() => onSelect("radio")} />
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
