/** Every screen of the interface, with what it needs to show. */
export type Screen =
  | { name: "welcome" }
  | { name: "createProfile" }
  | { name: "chats" }
  | { name: "chat"; chatId: string }
  | { name: "newChat" }
  | { name: "scan" }
  | { name: "request"; invitationId: string }
  | { name: "settings" }
  | { name: "account" }
  | { name: "radioSettings" }
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
