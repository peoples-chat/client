import { Navigator } from "./navigation/Navigator";
import type { Screen } from "./navigation/screens";
import { Account } from "./screens/Account";
import { ChatScreen } from "./screens/ChatScreen";
import { Chats } from "./screens/Chats";
import { CreateProfile } from "./screens/CreateProfile";
import { ExportKey } from "./screens/ExportKey";
import { Invite } from "./screens/Invite";
import { InvitedStart } from "./screens/InvitedStart";
import { InviteOne } from "./screens/InviteOne";
import { MessageRequest } from "./screens/MessageRequest";
import { NewChat } from "./screens/NewChat";
import { PairRadio } from "./screens/PairRadio";
import { Privacy } from "./screens/Privacy";
import { Profile } from "./screens/Profile";
import { RadioNotice } from "./screens/RadioNotice";
import { RadioSettings } from "./screens/RadioSettings";
import { Relays } from "./screens/Relays";
import { Scan } from "./screens/Scan";
import { Settings } from "./screens/Settings";
import { Welcome } from "./screens/Welcome";
import { useAppState } from "./state";
import { OpenedLinks } from "./navigation/OpenedLinks";
import layout from "./ui/layout.module.css";
import styles from "./App.module.css";

export default function App() {
  const state = useAppState();
  return (
    <div className={styles.app}>
      {state.status === "ready" && (
        <Navigator initial={state.account ? { name: "chats" } : { name: "welcome" }} alongside={<OpenedLinks />}>
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
    case "chat":
      return <ChatScreen chatId={screen.chatId} invited={screen.invited} />;
    case "newChat":
      return <NewChat link={screen.link} />;
    case "scan":
      return <Scan />;
    case "request":
      return <MessageRequest invitationId={screen.invitationId} />;
    case "settings":
      return <Settings />;
    case "account":
      return <Account />;
    case "exportKey":
      return <ExportKey />;
    case "radioSettings":
      return <RadioSettings />;
    case "pairRadio":
      return <PairRadio />;
    case "radioNotice":
      return <RadioNotice />;
    case "invite":
      return <Invite />;
    case "inviteOne":
      return <InviteOne />;
    case "invitedStart":
      return <InvitedStart link={screen.link} />;
    case "privacy":
      return <Privacy />;
    case "relays":
      return <Relays />;
    case "profile":
      return <Profile />;
  }
}
