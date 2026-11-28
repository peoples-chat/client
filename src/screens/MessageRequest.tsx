import { useRef, useState } from "react";
import { core, type Invitation } from "../core";
import { useNavigation } from "../navigation/Navigator";
import { useAppState } from "../state";
import { Button, IconButton } from "../ui/Button";
import { MessageList } from "../ui/MessageList";
import { firstName, tintFor } from "../ui/people";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText, Missing } from "./shared";
import styles from "./MessageRequest.module.css";

/**
 * What the sender learns from the request (#129): nothing. Marmot has no read receipts, and
 * accepting joins the chat without telling them.
 */
export function requestNote(name: string): string {
  return `${firstName(name)} is not told whether you read this or accept. They see only what you reply.`;
}

/** An invitation from someone who is not a contact yet. Only Accept is built so far. */
export function MessageRequest({ invitationId }: { invitationId: string }) {
  const { invitations } = useAppState();
  const navigation = useNavigation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Accepting removes the request before the chat replaces this screen; keep showing it meanwhile.
  const found = invitations.find((i) => i.id === invitationId);
  const lastSeen = useRef<Invitation | undefined>(found);
  if (found) lastSeen.current = found;
  const invitation = found ?? (busy ? lastSeen.current : undefined);

  if (!invitation) return <Missing title="Message request" text="This request is no longer available." />;
  const name = invitation.from.name;

  async function accept() {
    setBusy(true);
    setError(null);
    try {
      const chat = await core.acceptInvitation(invitationId);
      navigation.replace({ name: "chat", chatId: chat.id });
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  }

  return (
    <div className={layout.screen}>
      <ScreenHeader
        title={name}
        subtitle="Not in your contacts"
        avatar={{ name, tint: tintFor(name), photo: invitation.from.photo }}
        bordered
        onLeading={navigation.back}
        actions={<IconButton label="About this request" icon="info" disabled />}
      />
      <MessageList messages={invitation.messages} />
      <footer className={styles.footer}>
        <p className={styles.note} data-testid="request-note">
          {requestNote(name)}
        </p>
        {error && (
          <p className={styles.error} role="alert">
            {error}
          </p>
        )}
        <div className={styles.actions}>
          {/* Blocking and deleting requests are not built yet. */}
          <Button variant="danger" disabled>
            Block
          </Button>
          <Button variant="secondary" disabled>
            Delete
          </Button>
          <Button onClick={accept} disabled={busy}>
            Accept
          </Button>
        </div>
      </footer>
    </div>
  );
}
