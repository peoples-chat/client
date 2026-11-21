import { Icon, type IconName } from "./Icon";
import { initials, type Tint } from "./people";
import styles from "./Avatar.module.css";

// Initials sizes from the design, by avatar size.
const FONT_SIZES: Record<number, number> = { 36: 12, 40: 14, 44: 15, 52: 18, 60: 20, 88: 30, 96: 33 };

interface AvatarProps {
  name: string;
  size: number;
  tint?: Tint;
  /** Shown instead of initials, for chats that are not a person. */
  icon?: IconName;
}

/** Decorative: the name next to it says who it is. */
export function Avatar({ name, size, tint = "green", icon }: AvatarProps) {
  const letters = initials(name);
  const iconSize = Math.round(size * 0.46);
  return (
    <div
      aria-hidden="true"
      className={`${styles.avatar} ${styles[tint]}`}
      style={{ width: size, height: size, fontSize: FONT_SIZES[size] ?? Math.round(size * 0.34) }}
    >
      {icon ? <Icon name={icon} size={iconSize} /> : letters || <Icon name="person" size={iconSize} />}
    </div>
  );
}
