import type { ButtonHTMLAttributes } from "react";
import { Icon, type IconName } from "./Icon";
import styles from "./Button.module.css";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** `danger` is red text, as Block; `dangerSolid` confirms an action that cannot be undone. */
  variant?: "primary" | "secondary" | "danger" | "dangerSolid";
  size?: "large" | "small";
}

export function Button({ variant = "primary", size = "large", className, ...props }: ButtonProps) {
  return (
    <button
      type="button"
      {...props}
      className={[styles.button, styles[variant], styles[size], className].filter(Boolean).join(" ")}
    />
  );
}

interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> {
  /** Accessible name, since the button shows only an icon. */
  label: string;
  icon: IconName;
  iconSize?: number;
}

export function IconButton({ label, icon, iconSize = 24, className, ...props }: IconButtonProps) {
  return (
    <button
      type="button"
      aria-label={label}
      {...props}
      className={[styles.iconButton, className].filter(Boolean).join(" ")}
    >
      <Icon name={icon} size={iconSize} />
    </button>
  );
}
