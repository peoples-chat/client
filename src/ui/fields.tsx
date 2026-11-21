import { useId, type InputHTMLAttributes } from "react";
import { Icon } from "./Icon";
import styles from "./fields.module.css";

interface SearchFieldProps extends InputHTMLAttributes<HTMLInputElement> {
  /** Accessible name; the placeholder alone is not one. */
  label: string;
}

/** The rounded field with a magnifier, used for search and for pasting links. */
export function SearchField({ label, className, ...input }: SearchFieldProps) {
  return (
    <label className={[styles.search, input.disabled ? styles.disabled : "", className].filter(Boolean).join(" ")}>
      <Icon name="search" size={18} />
      <span className="visually-hidden">{label}</span>
      <input type="search" {...input} className={styles.searchInput} />
    </label>
  );
}

interface TextFieldProps extends InputHTMLAttributes<HTMLInputElement> {
  label: string;
  helper?: string;
}

export function TextField({ label, helper, id, ...input }: TextFieldProps) {
  const autoId = useId();
  const inputId = id ?? autoId;
  const helperId = helper ? `${inputId}-helper` : undefined;
  return (
    <div className={styles.field}>
      <label htmlFor={inputId} className={styles.label}>
        {label}
      </label>
      <input type="text" id={inputId} aria-describedby={helperId} {...input} className={styles.input} />
      {helper && (
        <span id={helperId} className={styles.helper}>
          {helper}
        </span>
      )}
    </div>
  );
}
