/** Every screen of the interface, with what it needs to show. */
export type Screen =
  | { name: "welcome" }
  | { name: "createProfile" }
  | { name: "chats" }
  /** `invited`: opened right after joining from the contact's invite link (#95). */
  | { name: "chat"; chatId: string; invited?: boolean }
  /** `link`: an invite link that opened the app, filled in for the user to confirm (#72). */
  | { name: "newChat"; link?: string }
  | { name: "scan" }
  | { name: "request"; invitationId: string }
  | { name: "settings" }
  | { name: "account" }
  | { name: "exportKey" }
  | { name: "radioSettings" }
  /** The USB ports a radio may be on, to pair it (#24). */
  | { name: "pairRadio" }
  | { name: "radioNotice" }
  | { name: "invite" }
  | { name: "inviteOne" }
  /** Someone without an account opened an invite link (#95). */
  | { name: "invitedStart"; link: string }
  | { name: "privacy" }
  | { name: "relays" }
  | { name: "profile" };

export function screenKey(screen: Screen): string {
  switch (screen.name) {
    case "chat":
      return `chat:${screen.chatId}`;
    case "request":
      return `request:${screen.invitationId}`;
    default:
      return screen.name;
  }
}
