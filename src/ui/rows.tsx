import type { ReactNode } from "react";
import { Avatar } from "./Avatar";
import { Icon, type IconName } from "./Icon";
import type { Tint } from "./people";
import styles from "./rows.module.css";

interface ActionRowProps {
  icon: IconName;
  label: string;
  /** A second line under the label. */
  description?: string;
  /** Action icons in green, as on New chat; settings rows keep them muted. */
  accent?: boolean;
  /** Red, for actions that cannot be undone. */
  danger?: boolean;
  /** A chevron for rows that open a screen; rows that act in place have none. */
  chevron?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  /** For tests: the row's `data-testid`. */
  testId?: string;
}

/** A row that opens something or does something. */
export function ActionRow({ icon, label, description, accent, danger, chevron = true, disabled, onClick, testId }: ActionRowProps) {
  return (
    <button
      type="button"
      data-testid={testId}
      className={[styles.actionRow, accent && styles.accent, danger && styles.danger].filter(Boolean).join(" ")}
      disabled={disabled}
      onClick={onClick}
    >
      <Icon name={icon} size={22} className={styles.actionIcon} />
      <span className={styles.actionText}>
        <span>{label}</span>
        {description && <span className={styles.actionDescription}>{description}</span>}
      </span>
      {chevron && <Icon name="chevron" size={18} className={styles.chevron} />}
    </button>
  );
}

/** A row that only tells something: the look of an action row, without the action. */
export function InfoRow({ icon, label, description }: { icon: IconName; label: string; description?: ReactNode }) {
  return (
    <div className={styles.actionRow}>
      <Icon name={icon} size={22} className={styles.actionIcon} />
      <span className={styles.actionText}>
        <span>{label}</span>
        {description && <span className={styles.actionDescription}>{description}</span>}
      </span>
    </div>
  );
}

interface SwitchRowProps {
  icon: IconName;
  label: string;
  description?: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}

/** A setting that is on or off, with the design's switch. The whole row toggles it. */
export function SwitchRow({ icon, label, description, checked, disabled, onChange }: SwitchRowProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      className={styles.actionRow}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <Icon name={icon} size={22} className={styles.actionIcon} />
      <span className={styles.actionText}>
        <span>{label}</span>
        {description && <span className={styles.actionDescription}>{description}</span>}
      </span>
      <span className={`${styles.switchTrack} ${checked ? styles.switchOn : ""}`} aria-hidden="true">
        <span className={styles.switchKnob} />
      </span>
    </button>
  );
}

interface ContactRowProps {
  name: string;
  tint: Tint;
  photo?: string;
  onClick?: () => void;
}

export function ContactRow({ name, tint, photo, onClick }: ContactRowProps) {
  return (
    <button type="button" className={styles.contactRow} onClick={onClick}>
      <Avatar name={name} size={44} tint={tint} photo={photo} />
      <span className={styles.contactName}>{name}</span>
    </button>
  );
}

interface ChatRowProps {
  name: string;
  tint: Tint;
  photo?: string;
  time: string;
  preview: ReactNode;
  /** A message request: its preview is highlighted. */
  request?: boolean;
  onClick: () => void;
}

export function ChatRow({ name, tint, photo, time, preview, request, onClick }: ChatRowProps) {
  return (
    <button type="button" className={styles.chatRow} onClick={onClick}>
      <Avatar name={name} size={52} tint={tint} photo={photo} />
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
