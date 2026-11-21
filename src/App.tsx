import { Navigator } from "./navigation/Navigator";
import type { Screen } from "./navigation/screens";
import { ChatScreen } from "./screens/ChatScreen";
import { Chats } from "./screens/Chats";
import { CreateProfile } from "./screens/CreateProfile";
import { MessageRequest } from "./screens/MessageRequest";
import { NewChat } from "./screens/NewChat";
import { Profile } from "./screens/Profile";
import { Radio } from "./screens/Radio";
import { Settings } from "./screens/Settings";
import { Missing } from "./screens/shared";
import { Welcome } from "./screens/Welcome";
import { useAppState } from "./state";
import layout from "./ui/layout.module.css";
import styles from "./App.module.css";

export default function App() {
  const state = useAppState();
  return (
    <div className={styles.app}>
      {state.status === "ready" && (
        <Navigator initial={state.account ? { name: "chats" } : { name: "welcome" }}>
          {(screen) => <ScreenView screen={screen} />}
        </Navigator>
      )}
      {state.status === "failed" && (
        <div className={layout.centered} role="alert">
          <p>peoples.chat could not start.</p>
          <p>{state.error}</p>
        </div>
      )}
    </div>
  );
}

function ScreenView({ screen }: { screen: Screen }) {
  switch (screen.name) {
    case "welcome":
      return <Welcome />;
    case "createProfile":
      return <CreateProfile />;
    case "chats":
      return <Chats />;
    case "radio":
      return <Radio />;
    case "chat":
      return <ChatScreen chatId={screen.chatId} />;
    case "newChat":
      return <NewChat />;
    case "scan":
      return <Missing title="Scan a QR code" text="Scanning QR codes works in the phone apps." />;
    case "request":
      return <MessageRequest invitationId={screen.invitationId} />;
    case "settings":
      return <Settings />;
    case "profile":
      return <Profile />;
  }
}
