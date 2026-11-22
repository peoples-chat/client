import { Fragment, useEffect, useLayoutEffect, useRef } from "react";
import type { DeliveryState, Message } from "../core/types";
import { formatDay, formatTime, sameDay } from "./format";
import { Icon } from "./Icon";
import styles from "./MessageList.module.css";

interface MessageListProps {
  messages: Message[];
  /** Shown when there are no messages yet. */
  empty?: string;
}

/** The messages of a chat, oldest at the top, with a separator for each day. */
export function MessageList({ messages, empty }: MessageListProps) {
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
      {messages.length === 0 && empty && <p className={styles.empty}>{empty}</p>}
      {messages.map((message, index) => (
        <Fragment key={message.id}>
          {(index === 0 || !sameDay(messages[index - 1].sentAt, message.sentAt)) && (
            <div className={styles.day}>{formatDay(message.sentAt, now)}</div>
          )}
          <MessageBubble message={message} />
        </Fragment>
      ))}
    </div>
  );
}

function MessageBubble({ message }: { message: Message }) {
  const outgoing = message.direction === "out";
  // Radio messages say so, in the radio colour (#48). Internet is the usual way and says nothing.
  const radio = message.transport === "radio";
  return (
    <div className={`${styles.bubble} ${outgoing ? styles.out : styles.in} ${radio ? styles.radio : ""}`}>
      <div className={styles.text}>{message.text}</div>
      <div className={styles.meta}>
        {radio && (
          <span className={styles.transport}>
            <Icon name="radio" size={12} strokeWidth={2.4} />
            Radio
          </span>
        )}
        <time dateTime={new Date(message.sentAt).toISOString()}>{formatTime(message.sentAt)}</time>
        {outgoing && message.delivery && <Delivery state={message.delivery} />}
      </div>
    </div>
  );
}

const DELIVERY_LABELS: Record<DeliveryState, string> = {
  waiting: "Waiting to send",
  sent: "Sent",
  confirmed: "Confirmed by the other radio",
};

function Delivery({ state }: { state: DeliveryState }) {
  return (
    <span role="img" aria-label={DELIVERY_LABELS[state]} className={styles.delivery}>
      <Icon name={state === "waiting" ? "clock" : "check"} size={15} strokeWidth={state === "waiting" ? 2 : 2.3} />
    </span>
  );
}
