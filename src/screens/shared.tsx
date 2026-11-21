import { CoreError } from "../core";
import { useNavigation } from "../navigation/Navigator";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";

/** Text for a failed command: the core's own reason, or a general one for unexpected errors. */
export function errorText(error: unknown): string {
  if (error instanceof CoreError) return error.message;
  console.error(error);
  return "Something went wrong. Try again.";
}

/** Shown when a screen's chat or request no longer exists. */
export function Missing({ title, text }: { title: string; text: string }) {
  const navigation = useNavigation();
  return (
    <div className={layout.screen}>
      <ScreenHeader title={title} onLeading={navigation.back} />
      <div className={layout.centered}>
        <p>{text}</p>
      </div>
    </div>
  );
}
