import { Fragment, useEffect, useLayoutEffect, useRef, type ReactNode } from "react";
import { radioBytes } from "../core/radio";
import { RADIO_TEXT_LIMIT, type DeliveryState, type Message } from "../core/types";
import { formatDay, formatTime, sameDay, shownText } from "./format";
import { Icon } from "./Icon";
import styles from "./MessageList.module.css";

interface MessageListProps {
  messages: Message[];
  /** Shown when there are no messages yet: a line of text, or a whole introduction. */
  empty?: ReactNode;
  /** Cancels a message that waits for a path (#46). Without it, there is nothing to cancel. */
  onCancel?: (messageId: string) => void;
  /** The chat's messages go by radio, so a waiting one too long for radio says why it waits. */
  byRadio?: boolean;
}

/** The messages of a chat, oldest at the top, with a separator for each day. */
export function MessageList({ messages, empty, onCancel, byRadio = false }: MessageListProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);

  // Keep the newest message in view as messages arrive.
  useLayoutEffect(() => {
    const list = listRef.current;
    if (list) list.scrollTop = list.scrollHeight;
  }, [messages.length]);

  // And when the list gets shorter, for example because the keyboard opened.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const onScroll = () => {
      atBottom.current = list.scrollHeight - list.scrollTop - list.clientHeight < 40;
    };
    const observer = new ResizeObserver(() => {
      if (atBottom.current) list.scrollTop = list.scrollHeight;
    });
    list.addEventListener("scroll", onScroll);
    observer.observe(list);
    return () => {
      list.removeEventListener("scroll", onScroll);
      observer.disconnect();
    };
  }, []);

  const now = Date.now();
  return (
    <div ref={listRef} className={styles.list} role="log" aria-label="Messages">
      <div className={styles.spacer} />
      {messages.length === 0 && (typeof empty === "string" ? <p className={styles.empty}>{empty}</p> : empty)}
      {messages.map((message, index) => (
        <Fragment key={message.id}>
          {(index === 0 || !sameDay(messages[index - 1].sentAt, message.sentAt)) && (
            <div className={styles.day}>{formatDay(message.sentAt, now)}</div>
          )}
          <MessageBubble message={message} />
          {message.withdrawn && !message.deleted && (
            <p
              className={`${styles.waiting} ${message.direction === "in" ? styles.noteIn : ""}`}
              data-testid="message-withdrawn-note"
            >
              {WITHDRAWN_NOTE}
            </p>
          )}
          {message.direction === "out" && message.delivery === "timedOut" && !message.withdrawn && (
            <p className={styles.waiting}>Not confirmed by radio. It goes by internet once a relay is back.</p>
          )}
          {message.cancellable && onCancel && (
            <WaitingNote
              tooLongForRadio={byRadio && radioBytes(message.text) > RADIO_TEXT_LIMIT}
              onCancel={() => onCancel(message.id)}
            />
          )}
        </Fragment>
      ))}
    </div>
  );
}

function MessageBubble({ message }: { message: Message }) {
  const outgoing = message.direction === "out";
  // Radio messages say so, in the radio colour (#48). Internet is the usual way and says nothing.
  const radio = message.transport === "radio";
  const state = message.deleted ? "deleted" : message.withdrawn ? "withdrawn" : message.edited ? "edited" : undefined;
  return (
    <div
      className={`${styles.bubble} ${outgoing ? styles.out : styles.in} ${radio ? styles.radio : ""} ${
        message.deleted ? styles.deleted : ""
      } ${message.withdrawn ? styles.withdrawn : ""}`}
      data-testid={`message-${message.id}`}
      data-state={state}
    >
      {message.sender && !outgoing && (
        <div className={styles.sender} data-testid="message-sender">
          {message.sender.name}
        </div>
      )}
      <div className={styles.text} data-testid="message-text">
        {shownText(message)}
      </div>
      <div className={styles.meta}>
        {radio && (
          <span className={styles.transport}>
            <Icon name="radio" size={12} strokeWidth={2.4} />
            Radio
          </span>
        )}
        {message.edited && !message.deleted && (
          <span className={styles.edited} data-testid="message-edited">
            Edited
          </span>
        )}
        <time dateTime={new Date(message.sentAt).toISOString()}>{formatTime(message.sentAt)}</time>
        {/* A withdrawn message never reached the chat, whatever the relay said (#122). */}
        {outgoing && message.delivery && !message.withdrawn && !message.deleted && (
          <Delivery state={message.delivery} radio={radio} />
        )}
        {message.withdrawn && !message.deleted && (
          <span role="img" aria-label="Not delivered" className={styles.delivery} data-testid="message-not-delivered">
            <Icon name="info" size={15} strokeWidth={2} />
          </span>
        )}
      </div>
    </div>
  );
}

/** Under a message that waits for a path: why, if it is too long for radio, and Cancel. */
function WaitingNote({ tooLongForRadio, onCancel }: { tooLongForRadio: boolean; onCancel: () => void }) {
  return (
    <div className={styles.waiting}>
      {tooLongForRadio && <span>Too long for radio. It waits for the internet.</span>}
      <button type="button" className={styles.cancel} onClick={onCancel}>
        Cancel
      </button>
    </div>
  );
}

/** Under a message that lost a race between two versions of the chat's state (#122). */
const WITHDRAWN_NOTE = "Not delivered. This message didn't reach the chat.";

const DELIVERY_LABELS: Record<DeliveryState, string> = {
  waiting: "Waiting to send",
  timedOut: "Not confirmed by the other radio",
  sent: "Sent",
  confirmed: "Confirmed by the other radio",
};

const DELIVERY_ICONS: Record<DeliveryState, "clock" | "help" | "check"> = {
  waiting: "clock",
  timedOut: "help",
  sent: "check",
  confirmed: "check",
};

/**
 * What is known about an outgoing message, and nothing more (#41). A radio message waits for the
 * other radio's confirmation once the radio has sent it.
 */
function Delivery({ state, radio }: { state: DeliveryState; radio: boolean }) {
  const label = state === "waiting" && radio ? "Sent by radio, waiting for the other radio to confirm" : DELIVERY_LABELS[state];
  const icon = DELIVERY_ICONS[state];
  return (
    <span role="img" aria-label={label} className={styles.delivery}>
      <Icon name={icon} size={15} strokeWidth={icon === "check" ? 2.3 : 2} />
    </span>
  );
}
