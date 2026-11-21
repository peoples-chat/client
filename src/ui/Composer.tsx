import { useRef, useState, type FormEvent } from "react";
import { IconButton } from "./Button";
import { Icon } from "./Icon";
import styles from "./Composer.module.css";

interface ComposerProps {
  /** Sends the text. Throws if it was not taken, and the text then stays in the field. */
  onSend: (text: string) => Promise<void>;
}

/**
 * The message field. Attachments, photos and voice messages are in the design but not built
 * yet, so their buttons are disabled. Send replaces camera and microphone once there is text.
 */
export function Composer({ onSend }: ComposerProps) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const hasText = text.trim().length > 0;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!hasText || sending) return;
    setSending(true);
    try {
      await onSend(text);
      setText("");
    } catch {
      // The screen shows why; the text stays so it can be sent again.
    } finally {
      setSending(false);
      inputRef.current?.focus();
    }
  }

  return (
    <form className={styles.composer} onSubmit={submit}>
      <IconButton label="Add an attachment" icon="plus" disabled />
      <label className={styles.field}>
        <span className="visually-hidden">Message</span>
        <input
          ref={inputRef}
          className={styles.input}
          type="text"
          placeholder="Message"
          value={text}
          onChange={(event) => setText(event.target.value)}
          enterKeyHint="send"
          autoComplete="off"
        />
      </label>
      {hasText ? (
        <button type="submit" className={styles.send} aria-label="Send" disabled={sending}>
          <Icon name="send" size={22} strokeWidth={2.2} />
        </button>
      ) : (
        <>
          <IconButton label="Take a photo" icon="camera" disabled />
          <IconButton label="Record a voice message" icon="mic" disabled />
        </>
      )}
    </form>
  );
}
