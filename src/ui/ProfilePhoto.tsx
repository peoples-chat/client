import { useRef } from "react";
import { Avatar } from "./Avatar";
import { Icon } from "./Icon";
import styles from "./ProfilePhoto.module.css";

interface ProfilePhotoProps {
  name: string;
  size: number;
  photo?: string;
  /** Called with the photo the user picked. Without it, the button is disabled. */
  onChoose?: (file: File) => void;
  busy?: boolean;
}

/**
 * The user's avatar with the photo button (#94). The button opens the phone's own picker, which
 * offers the photo library and the camera.
 */
export function ProfilePhoto({ name, size, photo, onChoose, busy }: ProfilePhotoProps) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <div className={styles.wrap} style={{ width: size, height: size }}>
      <Avatar name={name} size={size} tint="green" photo={photo} />
      <button
        type="button"
        className={styles.photoButton}
        aria-label="Choose a photo"
        disabled={!onChoose || busy}
        onClick={() => input.current?.click()}
      >
        <Icon name="camera" size={18} />
      </button>
      {onChoose && (
        <input
          ref={input}
          type="file"
          accept="image/*"
          hidden
          onChange={(event) => {
            const file = event.target.files?.[0];
            // The same photo again still counts as a choice.
            event.target.value = "";
            if (file) onChoose(file);
          }}
        />
      )}
    </div>
  );
}
