// What a chat shows for a message its sender edited or deleted, and for one that never reached
// the chat (#122). Rendered to markup, without a browser.
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Message } from "../core/types";
import { MessageList } from "./MessageList";

const message = (id: string, extra: Partial<Message> = {}): Message => ({
  id,
  direction: "in",
  text: `Text of ${id}`,
  sentAt: 1_795_300_000_000,
  transport: "internet",
  ...extra,
});

/** The markup of a chat with the one message `shown`. */
function rendered(shown: Message): string {
  const html = renderToStaticMarkup(<MessageList messages={[shown]} />);
  expect(html).toContain(`data-testid="message-${shown.id}"`);
  return html;
}

describe("issue 122: MessageList", () => {
  it("shows an edited message with its new text and an edited mark", () => {
    const html = rendered(message("m1", { text: "Meet at the new mill", edited: true }));
    expect(html).toContain('data-state="edited"');
    expect(html).toContain("Meet at the new mill");
    expect(html).toContain('data-testid="message-edited"');
  });

  it("shows a deleted message as deleted, without text or an edited mark", () => {
    const html = rendered(message("m1", { text: "", deleted: true, edited: true }));
    expect(html).toContain('data-state="deleted"');
    expect(html).toContain("This message was deleted.");
    expect(html).not.toContain('data-testid="message-edited"');
  });

  it("shows a withdrawn message of the user's as not delivered, not as sent", () => {
    const html = rendered(message("m1", { direction: "out", delivery: "sent", withdrawn: true, text: "See you there." }));
    expect(html).toContain('data-state="withdrawn"');
    expect(html).toContain("See you there.");
    expect(html).toContain('data-testid="message-not-delivered"');
    expect(html).toContain('data-testid="message-withdrawn-note"');
    expect(html).not.toContain('aria-label="Sent"');
  });

  it("shows a plain message as it was", () => {
    const html = rendered(message("m1", { direction: "out", delivery: "sent" }));
    expect(html).not.toContain("data-state=");
    expect(html).toContain('aria-label="Sent"');
    expect(html).not.toContain('data-testid="message-not-delivered"');
  });
});
