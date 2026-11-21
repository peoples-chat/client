import type { ReactNode } from "react";
import { Avatar } from "./Avatar";
import { Icon, type IconName } from "./Icon";
import type { Tint } from "./people";
import styles from "./rows.module.css";

interface ActionRowProps {
  icon: IconName;
  label: string;
  /** Action icons in green, as on New chat; settings rows keep them muted. */
  accent?: boolean;
  /** A chevron for rows that open a screen; rows that act in place have none. */
  chevron?: boolean;
  disabled?: boolean;
  onClick?: () => void;
}

/** A row that opens something or does something. */
export function ActionRow({ icon, label, accent, chevron = true, disabled, onClick }: ActionRowProps) {
  return (
    <button
      type="button"
      className={`${styles.actionRow} ${accent ? styles.accent : ""}`}
      disabled={disabled}
      onClick={onClick}
    >
      <Icon name={icon} size={22} className={styles.actionIcon} />
      <span className={styles.actionLabel}>{label}</span>
      {chevron && <Icon name="chevron" size={18} className={styles.chevron} />}
    </button>
  );
}

interface ContactRowProps {
  name: string;
  tint: Tint;
  onClick?: () => void;
}

export function ContactRow({ name, tint, onClick }: ContactRowProps) {
  return (
    <button type="button" className={styles.contactRow} onClick={onClick}>
      <Avatar name={name} size={44} tint={tint} />
      <span className={styles.contactName}>{name}</span>
    </button>
  );
}

interface ChatRowProps {
  name: string;
  tint: Tint;
  time: string;
  preview: ReactNode;
  /** A message request: its preview is highlighted. */
  request?: boolean;
  onClick: () => void;
}

export function ChatRow({ name, tint, time, preview, request, onClick }: ChatRowProps) {
  return (
    <button type="button" className={styles.chatRow} onClick={onClick}>
      <Avatar name={name} size={52} tint={tint} />
      <span className={styles.chatBody}>
        <span className={styles.chatTop}>
          <span className={styles.chatName}>{name}</span>
          <span className={styles.chatTime}>{time}</span>
        </span>
        <span className={`${styles.chatPreview} ${request ? styles.request : ""}`}>{preview}</span>
      </span>
    </button>
  );
}

export function SectionTitle({ children }: { children: ReactNode }) {
  return <h2 className={styles.sectionTitle}>{children}</h2>;
}
