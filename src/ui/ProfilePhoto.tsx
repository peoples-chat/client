import { Avatar } from "./Avatar";
import { Icon } from "./Icon";
import styles from "./ProfilePhoto.module.css";

/** The user's avatar with the photo button. Photos are not built yet, so the button is disabled. */
export function ProfilePhoto({ name, size }: { name: string; size: number }) {
  return (
    <div className={styles.wrap} style={{ width: size, height: size }}>
      <Avatar name={name} size={size} tint="green" />
      <button type="button" className={styles.photoButton} aria-label="Choose a photo" disabled>
        <Icon name="camera" size={18} />
      </button>
    </div>
  );
}
