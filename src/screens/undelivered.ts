import type { UndeliveredInvitation } from "../core/types";

/** What a chat says while the contact hasn't received the invitation to it (#126). */
export function undeliveredText(undelivered: UndeliveredInvitation, first: string): string {
  const notYet = `${first} hasn't received your invitation yet, so they can't see this chat.`;
  return undelivered.reason === "tooLarge"
    ? `${notYet} A relay refused it as too large. The app tries again when relays change.`
    : `${notYet} The app keeps trying.`;
}
