import styles from "./Logo.module.css";

/** The peoples.chat logo: two people joined by a radio wave, on the green tile. Decorative. */
export function Logo() {
  return (
    <div className={styles.tile} aria-hidden="true">
      <svg width="64" height="64" viewBox="0 0 100 100">
        <circle cx="14" cy="50" r="9" fill="currentColor" />
        <circle cx="86" cy="50" r="9" fill="currentColor" />
        <path
          d="M31 50C38 22 44 22 50 50S62 78 69 50"
          fill="none"
          stroke="currentColor"
          strokeWidth="9"
          strokeLinecap="round"
        />
      </svg>
    </div>
  );
}
