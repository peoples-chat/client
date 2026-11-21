import type { ButtonHTMLAttributes, ReactNode } from "react";
import styles from "./Chip.module.css";

interface ChipProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  selected?: boolean;
  children: ReactNode;
}

/** A filter toggle. The button is 44px tall for touch; the visible pill is 32px. */
export function Chip({ selected = false, children, ...props }: ChipProps) {
  return (
    <button type="button" aria-pressed={selected} {...props} className={styles.chip}>
      <span className={`${styles.pill} ${selected ? styles.selected : ""}`}>{children}</span>
    </button>
  );
}

export function ChipRow({ children }: { children: ReactNode }) {
  return <div className={styles.row}>{children}</div>;
}
