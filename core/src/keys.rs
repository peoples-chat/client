//! Where account keys live (#115).
//!
//! Normally they live in the platform's key store: the keychain on Apple devices, the Keystore on
//! Android. Development builds on computers keep them in a file in the data folder instead. The
//! Mac's keychain sees each rebuild as a new app, so it would ask for the password at every start.
//! Release builds and phones keep the platform's store.
//!
//! A key lives in one place at a time. When a build keeps keys somewhere other than where the
//! build before kept them, each key moves once, at the build's first start: into the file for a
//! development build, back into the keychain for one that uses the keychain. Only the file is asked
//! where a key is, since asking the keychain can bring up its password prompt.

use std::path::Path;
use std::sync::Arc;

use marmot_account::{
    AccountHome, AccountHomeError, AccountHomeResult, AccountSecretStore, AccountSummary,
    KeychainSecretStore, LocalFileSecretStore,
};

/// Opens the account home in `data_dir`. Account keys live in files when `keys_in_file`, and
/// otherwise in the platform's key store under `keychain_service`. Each key that is in the other
/// place moves first.
///
/// A key that can't move, for example because the keychain's prompt was declined, stays where it
/// is. The home then uses that place, so that the account keeps working.
pub(crate) fn open_account_home(
    data_dir: &Path,
    keychain_service: &str,
    keys_in_file: bool,
) -> AccountHomeResult<AccountHome> {
    let file: Arc<dyn AccountSecretStore> = Arc::new(LocalFileSecretStore::new(data_dir));
    let keychain: Arc<dyn AccountSecretStore> =
        Arc::new(KeychainSecretStore::new(keychain_service)?);
    let accounts: Vec<AccountSummary> = AccountHome::open_with_secret_store(data_dir, file.clone())
        .accounts()?
        .into_iter()
        .filter(|account| account.local_signing)
        .collect();
    let (wanted, other) = if keys_in_file {
        (file.clone(), keychain)
    } else {
        (keychain, file.clone())
    };
    let mut stuck = false;
    for account in &accounts {
        let in_file = file.has_secret_for_label(&account.label)?;
        if in_file == keys_in_file {
            continue;
        }
        if let Err(error) = move_key(account, other.as_ref(), wanted.as_ref()) {
            log::warn!("an account key could not move, so it stays where it was: {error}");
            stuck = true;
        }
    }
    let store = if stuck { other } else { wanted };
    Ok(AccountHome::open_with_secret_store(data_dir, store))
}

/// Moves `account`'s key from one store to the other. It is removed from `from` only once `to`
/// reads it back. The move counts once `to` has it. If the old copy can't be removed, it stays
/// behind, and the log says so.
fn move_key(
    account: &AccountSummary,
    from: &dyn AccountSecretStore,
    to: &dyn AccountSecretStore,
) -> AccountHomeResult<()> {
    let keys = from.load_secret(account)?;
    to.write_secret(account, &keys)?;
    if to.load_secret(account)?.public_key() != keys.public_key() {
        return Err(AccountHomeError::SecretStore(
            "the moved key reads back differently".into(),
        ));
    }
    match from.remove_secret(account) {
        Ok(()) => log::info!("moved an account key to where this build keeps keys"),
        Err(error) => log::warn!("moved an account key, but its old copy stays behind: {error}"),
    }
    Ok(())
}
