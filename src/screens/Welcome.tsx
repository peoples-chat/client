import { useNavigation } from "../navigation/Navigator";
import { Button } from "../ui/Button";
import { Logo } from "../ui/Logo";
import layout from "../ui/layout.module.css";
import styles from "./Welcome.module.css";

export function Welcome() {
  const navigation = useNavigation();
  return (
    <div className={layout.screen}>
      <main className={styles.main}>
        <Logo />
        <h1 className={styles.title}>peoples.chat</h1>
        <p className={styles.tagline}>Private messages that keep working when the internet does not.</p>
      </main>
      <footer className={styles.footer}>
        <Button onClick={() => navigation.push({ name: "createProfile" })}>Create account</Button>
        {/* Recovery is not built yet (#63). */}
        <Button variant="secondary" disabled>
          I already have an account
        </Button>
        <p className={styles.note}>No phone number or email needed.</p>
      </footer>
    </div>
  );
}
