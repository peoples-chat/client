import { useRef, useState, type FormEvent } from "react";
import { radioBytes } from "../core/radio";
import { RADIO_TEXT_LIMIT } from "../core/types";
import { IconButton } from "./Button";
import { Icon } from "./Icon";
import styles from "./Composer.module.css";

interface ComposerProps {
  /** Sends the text. Throws if it was not taken, and the text then stays in the field. */
  onSend: (text: string) => Promise<void>;
  /**
   * `radio` while the chat's messages go by radio (#48): short text only, with a byte counter
   * against the radio limit, and a send button in the radio colour.
   */
  mode?: "internet" | "radio";
  /**
   * In radio mode, longer text can still be sent, and waits for the internet (#46). Not while
   * radio is forced: the core refuses longer text then.
   */
  longWaits?: boolean;
}

/**
 * The message field. Attachments, photos and voice messages are in the design but not built
 * yet, so their buttons are disabled. Send replaces camera and microphone once there is text.
 */
export function Composer({ onSend, mode = "internet", longWaits = false }: ComposerProps) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const hasText = text.trim().length > 0;
  const radio = mode === "radio";
  const bytes = radio ? radioBytes(text) : 0;
  const tooLong = radio && bytes > RADIO_TEXT_LIMIT;
  // Too long for radio, it may still go, and waits for the internet.
  const waits = tooLong && longWaits;
  const blocked = tooLong && !longWaits;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!hasText || sending || blocked) return;
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
      <IconButton label={radio ? "Attachments wait for internet" : "Add an attachment"} icon="plus" disabled />
      <label className={styles.field}>
        <span className="visually-hidden">Message</span>
        <input
          ref={inputRef}
          className={styles.input}
          type="text"
          placeholder={radio ? "Short text by radio" : "Message"}
          value={text}
          onChange={(event) => setText(event.target.value)}
          enterKeyHint="send"
          autoComplete="off"
          aria-describedby={radio && hasText ? "radio-count" : undefined}
          aria-invalid={tooLong || undefined}
        />
      </label>
      {radio && hasText && (
        <span id="radio-count" className={`${styles.count} ${blocked ? styles.tooLong : ""}`} aria-live="polite">
          <span aria-hidden="true">
            {waits ? "Waits for internet" : `${bytes}/${RADIO_TEXT_LIMIT}`}
          </span>
          <span className="visually-hidden">
            {waits
              ? `Too long for radio: ${bytes} of ${RADIO_TEXT_LIMIT} bytes. It waits for the internet.`
              : tooLong
                ? `Too long for radio: ${bytes} of ${RADIO_TEXT_LIMIT} bytes`
                : `${bytes} of ${RADIO_TEXT_LIMIT} bytes for radio`}
          </span>
        </span>
      )}
      {/* On radio there are no photos or voice messages, so the send button stays, as in the
          design's radio chat. */}
      {hasText || radio ? (
        <button
          type="submit"
          className={`${styles.send} ${radio && !waits ? styles.radio : ""} ${blocked ? styles.tooLongSend : ""}`}
          aria-label={waits ? "Send when the internet is back" : radio ? "Send by radio" : "Send"}
          disabled={!hasText || sending || blocked}
        >
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
