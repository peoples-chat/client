/** Every screen of the interface, with what it needs to show. */
export type Screen =
  | { name: "welcome" }
  | { name: "createProfile" }
  | { name: "chats" }
  | { name: "chat"; chatId: string }
  /** `link`: an invite link that opened the app, filled in for the user to confirm (#72). */
  | { name: "newChat"; link?: string }
  | { name: "scan" }
  | { name: "request"; invitationId: string }
  | { name: "settings" }
  | { name: "account" }
  | { name: "exportKey" }
  | { name: "radioSettings" }
  | { name: "radioNotice" }
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
