// Issue 123: every message shows something, also the ones the app can't render yet.
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Message } from "../core/types";
import { MessageBubble } from "./MessageList";
import { messageSummary, quoteSummary } from "./messageContent";

const message = (extra: Partial<Message>): Message => ({
  id: "m1",
  direction: "in",
  text: "",
  sentAt: Date.UTC(2026, 10, 28, 12),
  transport: "internet",
  ...extra,
});

/** The bubble as static HTML. The tests run without a DOM, so they read it by `data-testid`. */
function render(shown: Message): string {
  return renderToStaticMarkup(<MessageBubble message={shown} contactName="Bea Holm" />);
}

const decode = (text: string) =>
  text.replace(/&#x27;/g, "'").replace(/&quot;/g, '"').replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");

/** Each element with `data-testid="id"`: its text, or the value of `attribute`. */
function byTestId(html: string, id: string, attribute?: string): string[] {
  const found: string[] = [];
  const opening = new RegExp(`<([a-z]+)([^>]*)data-testid="${id}"([^>]*)>`, "g");
  for (const match of html.matchAll(opening)) {
    if (attribute) {
      const value = new RegExp(`${attribute}="([^"]*)"`).exec(match[2] + match[3]);
      found.push(decode(value?.[1] ?? ""));
      continue;
    }
    // Find the element's end: its own closing tag, counting nested ones of the same name.
    const tag = match[1];
    const tags = new RegExp(`<(/?)${tag}\\b[^>]*>`, "g");
    tags.lastIndex = match.index + match[0].length;
    let depth = 1;
    let end = html.length;
    for (const next of html.matchAll(tags)) {
      if (next.index < tags.lastIndex) continue;
      depth += next[1] ? -1 : 1;
      if (depth === 0) {
        end = next.index;
        break;
      }
    }
    found.push(decode(html.slice(match.index + match[0].length, end).replace(/<[^>]*>/g, "")));
  }
  return found;
}

describe("issue 123: messages the app can't show", () => {
  it("names a photo and shows its caption", () => {
    const shown = render(message({ placeholder: "photo", text: "The view from the hill" }));
    expect(byTestId(shown, "message-placeholder")).toEqual(["Photopeoples.chat can't show this yet"]);
    expect(byTestId(shown, "message-text")).toEqual(["The view from the hill"]);
  });

  it("names stickers, polls and unknown messages, without empty text", () => {
    for (const [placeholder, label] of [
      ["sticker", "Sticker"],
      ["poll", "Poll"],
      ["unsupported", "Unsupported message"],
    ] as const) {
      const shown = render(message({ placeholder }));
      expect(byTestId(shown, "message-placeholder")[0]).toContain(label);
      expect(byTestId(shown, "message-text")).toEqual([]);
    }
  });

  it("shows a chat event as a line, not a bubble", () => {
    const shown = render(message({ placeholder: "chatEvent", text: "Bea left the chat" }));
    expect(byTestId(shown, "message-chat-event")).toEqual(["Bea left the chat"]);
    expect(byTestId(shown, "message")).toEqual([]);
  });

  it("shows reactions on their message, with a count", () => {
    const shown = render(
      message({
        text: "Shall we meet?",
        direction: "out",
        delivery: "sent",
        reactions: [
          { emoji: "❤️", count: 2, mine: true },
          { emoji: "👍", count: 1, mine: false },
        ],
      }),
    );
    expect(byTestId(shown, "message-reaction")).toEqual(["❤️2", "👍1"]);
    expect(byTestId(shown, "message-reaction", "aria-label")).toEqual(["You and 1 more reacted with ❤️", "1 person reacted with 👍"]);
  });

  it("shows the quote a reply answers, and says when it is not available", () => {
    const mine = render(
      message({ text: "Three works", replyTo: { messageId: "m0", direction: "out", text: "Shall we meet at three?" } }),
    );
    expect(byTestId(mine, "message-quote")).toEqual(["YouShall we meet at three?"]);
    expect(byTestId(mine, "message-text")).toEqual(["Three works"]);

    const theirs = render(message({ text: "Nice", replyTo: { messageId: "m0", direction: "in", text: "", placeholder: "photo" } }));
    expect(byTestId(theirs, "message-quote")).toEqual(["Bea HolmPhoto"]);

    const gone = render(message({ text: "What?", replyTo: { messageId: "m0", text: "" } }));
    expect(byTestId(gone, "message-quote")).toEqual(["Message not available"]);
  });

  it("sums a message up in one line for the chat list", () => {
    expect(messageSummary(message({ text: "Hello" }))).toBe("Hello");
    expect(messageSummary(message({ placeholder: "photo", text: "Look" }))).toBe("Photo: Look");
    expect(messageSummary(message({ placeholder: "sticker" }))).toBe("Sticker");
    expect(messageSummary(message({ placeholder: "chatEvent", text: "" }))).toBe("Chat update");
    expect(quoteSummary({ messageId: "m0", text: "" })).toBe("Message not available");
  });
});
