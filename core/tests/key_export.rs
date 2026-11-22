//! Exporting the account key (#86): a password-protected NIP-49 copy that opens with its
//! password, or the plain key, each only once the device's owner has confirmed. The key comes
//! from the in-memory keychain, never the machine's own.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};

use nostr::nips::nip19::{FromBech32, ToBech32};
use nostr::nips::nip49::{EncryptedSecretKey, KeySecurity};
use nostr::prelude::SecretKey;
use peoples_chat_core::interface::CommandError;
use support::{Phone, TestRelay};

const PASSWORD: &str = "correct horse battery";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_account_key_exports_after_the_owner_confirms() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let phone = Phone::new("export", &relay.url());
    let core = phone.start().await;
    let confirmed = AtomicUsize::new(0);
    let confirm = || async {
        confirmed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };

    // Nothing to export yet, and nothing is asked.
    let error = core
        .export_account_key(Some(PASSWORD), confirm)
        .await
        .unwrap_err();
    assert_eq!(error.message, "Create an account first.");
    let account = core.create_account("Ada Lund").await.unwrap();
    let secret = keyring_core::Entry::new("chat.peoples.client.test-export", &account.public_key)
        .unwrap()
        .get_password()
        .unwrap();
    let secret = SecretKey::parse(&secret).unwrap();

    // A protected copy opens with its password, and only with it.
    let copy = core
        .export_account_key(Some(PASSWORD), confirm)
        .await
        .unwrap();
    assert!(copy.starts_with("ncryptsec1"), "{}", &copy[..10]);
    let encrypted = EncryptedSecretKey::from_bech32(&copy).unwrap();
    assert_eq!(encrypted.decrypt(PASSWORD).unwrap(), secret);
    assert!(encrypted.decrypt("correct horse batterY").is_err());
    // MDK doesn't track how the key was handled until a plain copy shows it.
    assert_eq!(encrypted.key_security(), KeySecurity::Unknown);

    // The plain key is the key itself, and from then on protected copies say it was exposed.
    let plain = core.export_account_key(None, confirm).await.unwrap();
    assert_eq!(*plain, secret.to_bech32().unwrap());
    let again = core
        .export_account_key(Some(PASSWORD), confirm)
        .await
        .unwrap();
    let again = EncryptedSecretKey::from_bech32(&again).unwrap();
    assert_eq!(again.key_security(), KeySecurity::Weak);
    assert_eq!(confirmed.load(Ordering::SeqCst), 3);

    // A short password is refused before the owner is asked.
    let error = core
        .export_account_key(Some("short words"), confirm)
        .await
        .unwrap_err();
    assert_eq!(error.message, "Use a password of at least 12 characters.");
    assert_eq!(confirmed.load(Ordering::SeqCst), 3);

    // When the owner doesn't confirm, nothing is exported.
    let refused = || async { Err(CommandError::new("Unlock your device to export the key.")) };
    let error = core.export_account_key(None, refused).await.unwrap_err();
    assert_eq!(error.message, "Unlock your device to export the key.");
    core.close().await.unwrap();
}
