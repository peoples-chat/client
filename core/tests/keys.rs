//! Where account keys live (#115). Development builds on computers keep them in a file in the data
//! folder, so that the Mac's keychain doesn't ask for its password at every start. A key moves
//! there from the keychain once, and moves back when a build uses the keychain again. These tests
//! use the mock keychain.

mod support;

use std::path::{Path, PathBuf};

use marmot_account::{AccountSecretStore, KeychainSecretStore};
use peoples_chat_core::Core;
use support::{Phone, TestRelay};

/// The account's key as a plain export, confirmed at once.
async fn exported(core: &Core) -> String {
    core.export_account_key(None, || async { Ok(()) })
        .await
        .unwrap()
        .to_string()
}

/// The key files in a data folder: one for each account whose key is kept in a file.
fn key_files(data_dir: &Path) -> Vec<PathBuf> {
    let Ok(accounts) = std::fs::read_dir(data_dir.join("accounts")) else {
        return Vec::new();
    };
    accounts
        .filter_map(Result::ok)
        .map(|account| account.path().join("secret.json"))
        .filter(|file| file.exists())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_key_moves_into_its_file_and_back() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;

    // A build that keeps keys in the keychain makes the account.
    let phone = Phone::new("keys-move", &relay.url());
    let keychain = KeychainSecretStore::new(phone.keychain_service()).unwrap();
    let core = phone.start().await;
    let account = core.create_account("Dana Krog").await.unwrap();
    let key = exported(&core).await;
    core.close().await.unwrap();
    assert!(
        keychain
            .has_secret_for_account_id(&account.public_key)
            .unwrap()
    );
    assert!(key_files(phone.data_dir()).is_empty());

    // A development build keeps keys in a file. The key moves there, and the account works.
    let phone = phone.with_keys_in_file(true);
    let core = phone.start().await;
    assert_eq!(
        core.snapshot().account.map(|account| account.public_key),
        Some(account.public_key.clone())
    );
    assert_eq!(exported(&core).await, key);
    core.close().await.unwrap();
    assert!(
        !keychain
            .has_secret_for_account_id(&account.public_key)
            .unwrap()
    );
    assert_eq!(key_files(phone.data_dir()).len(), 1);

    // It stays there across a restart.
    let core = phone.start().await;
    assert_eq!(exported(&core).await, key);
    core.close().await.unwrap();
    assert_eq!(key_files(phone.data_dir()).len(), 1);

    // A build that uses the keychain again moves it back.
    let phone = phone.with_keys_in_file(false);
    let core = phone.start().await;
    assert_eq!(exported(&core).await, key);
    core.close().await.unwrap();
    assert!(
        keychain
            .has_secret_for_account_id(&account.public_key)
            .unwrap()
    );
    assert!(key_files(phone.data_dir()).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_account_made_in_a_development_build_keeps_its_key_in_a_file_until_deleted() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let phone = Phone::new("keys-file", &relay.url()).with_keys_in_file(true);
    let keychain = KeychainSecretStore::new(phone.keychain_service()).unwrap();
    let core = phone.start().await;
    let account = core.create_account("Eik Holm").await.unwrap();
    assert_eq!(key_files(phone.data_dir()).len(), 1);
    assert!(
        !keychain
            .has_secret_for_account_id(&account.public_key)
            .unwrap()
    );

    // Deleting the account removes its key file too.
    core.delete_account().await.unwrap();
    assert!(key_files(phone.data_dir()).is_empty());
    assert!(
        !keychain
            .has_secret_for_account_id(&account.public_key)
            .unwrap()
    );
    core.close().await.unwrap();
}
