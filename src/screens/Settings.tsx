import { useEffect, useState } from "react";
import { usesDemoData } from "../core";
import { coreVersion } from "../core/shell";
import { useNavigation } from "../navigation/Navigator";
import type { Screen } from "../navigation/screens";
import { useAppState } from "../state";
import { Avatar } from "../ui/Avatar";
import { Icon, type IconName } from "../ui/Icon";
import { ActionRow } from "../ui/rows";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import styles from "./Settings.module.css";

// Every setting in the design. Those without a screen are not built yet and are disabled (#63).
const SETTINGS: { icon: IconName; label: string; screen?: Screen }[] = [
  { icon: "person", label: "Account", screen: { name: "account" } },
  { icon: "devices", label: "Linked devices" },
  { icon: "group", label: "Recovery" },
  { icon: "radio", label: "Radio", screen: { name: "radioSettings" } },
  { icon: "moon", label: "Appearance" },
  { icon: "chat", label: "Chats and storage" },
  { icon: "bell", label: "Notifications" },
  { icon: "lock", label: "Privacy", screen: { name: "privacy" } },
  { icon: "help", label: "Help" },
  { icon: "personAdd", label: "Invite friends" },
];

export function Settings() {
  const { account } = useAppState();
  const navigation = useNavigation();
  const core = useCoreLink();

  return (
    <div className={layout.screen}>
      <ScreenHeader title="Settings" onLeading={navigation.back} />
      <main className={layout.scroll}>
        <button type="button" className={styles.profile} onClick={() => navigation.push({ name: "profile" })}>
          <Avatar name={account?.name ?? ""} size={60} tint="green" photo={account?.photo} />
          <span className={styles.profileText}>
            <span className={styles.name}>{account?.name}</span>
            <span className={styles.subtitle}>Your code and link</span>
          </span>
          <Icon name="qr" strokeWidth={1.8} className={styles.qr} />
        </button>
        {SETTINGS.map(({ icon, label, screen }) => (
          <ActionRow
            key={label}
            icon={icon}
            label={label}
            disabled={!screen}
            onClick={screen && (() => navigation.push(screen))}
          />
        ))}
        <p className={styles.version}>
          {core}
          {usesDemoData && " · demo data"}
        </p>
      </main>
    </div>
  );
}

/** "Core 0.1.0" when the app shell answers, which shows the web view, shell and core are wired up. */
function useCoreLink(): string {
  const [text, setText] = useState("");
  useEffect(() => {
    let active = true;
    coreVersion().then(
      (version) => active && setText(version === null ? "Browser preview" : `Core ${version}`),
      () => active && setText("Core not reachable"),
    );
    return () => {
      active = false;
    };
  }, []);
  return text;
}
