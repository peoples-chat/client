import type { ReactNode } from "react";
import { Avatar } from "./Avatar";
import { IconButton } from "./Button";
import type { Tint } from "./people";
import styles from "./ScreenHeader.module.css";

interface ScreenHeaderProps {
  title: string;
  subtitle?: ReactNode;
  /** A pushed screen goes back; a screen opened like a sheet closes. */
  leading?: "back" | "close";
  onLeading: () => void;
  avatar?: { name: string; tint: Tint; photo?: string };
  actions?: ReactNode;
  /** Chats and requests separate the header from the messages with a line. */
  bordered?: boolean;
}

/** Header of a screen pushed on top of a tab. */
export function ScreenHeader({ title, subtitle, leading = "back", onLeading, avatar, actions, bordered }: ScreenHeaderProps) {
  return (
    <header className={`${styles.header} ${bordered ? styles.bordered : ""}`}>
      <IconButton label={leading === "back" ? "Back" : "Close"} icon={leading} onClick={onLeading} />
      {avatar && <Avatar name={avatar.name} size={40} tint={avatar.tint} photo={avatar.photo} />}
      <div className={styles.titles}>
        <h1 className={styles.title}>{title}</h1>
        {subtitle && <div className={styles.subtitle}>{subtitle}</div>}
      </div>
      {actions}
    </header>
  );
}

interface TabHeaderProps {
  title: string;
  accountName: string;
  accountPhoto?: string;
  onOpenSettings: () => void;
  action?: ReactNode;
}

/** Header of a tab: the user's avatar leads to settings, then the title and one action. */
export function TabHeader({ title, accountName, accountPhoto, onOpenSettings, action }: TabHeaderProps) {
  return (
    <header className={styles.tabHeader}>
      <button type="button" className={styles.profileButton} aria-label="Settings and profile" onClick={onOpenSettings}>
        <Avatar name={accountName} size={36} tint="green" photo={accountPhoto} />
      </button>
      <h1 className={styles.tabTitle}>{title}</h1>
      {action}
    </header>
  );
}
