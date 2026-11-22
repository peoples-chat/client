//! Internet messaging: Marmot over Nostr relays, through MDK's multi-account app runtime.
//!
//! This is the link to MDK (#59): open the runtime, create, list and delete accounts, and wait
//! until an account's relay lists, profile and KeyPackage are on the relays. [`crate::Core`]
//! builds the interface's commands and events on it (#16); signing through a signer interface
//! comes with #17.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cgka_traits::TransportEndpoint;
use marmot_account::AccountHome;
use marmot_app::{
    AccountSetupReadiness, AccountSetupRequest, AppError, MarmotApp, MarmotAppConfig,
    MarmotAppRuntime, UserProfileMetadata,
};

/// How often [`Messaging::wait_until_published`] checks the account's setup state.
const READINESS_POLL: Duration = Duration::from_millis(50);

/// Where the core keeps its messaging data and which relays it starts with.
#[derive(Clone, Debug)]
pub struct MessagingOptions {
    /// Folder for MDK's encrypted databases. One installation of the app uses one folder, and
    /// only one runtime at a time may open it.
    pub data_dir: PathBuf,
    /// Relays for new accounts: their relay lists, profile and KeyPackage are published here.
    pub relays: Vec<String>,
    /// Keychain service under which account keys are stored, one entry per account.
    pub keychain_service: String,
    /// Allows `ws://` relays on the loopback interface, for tests and local development.
    pub allow_loopback_relays: bool,
}

/// An account created on this device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAccount {
    /// The account's public key, in hex. MDK also uses it as the account's label.
    pub public_key_hex: String,
}

/// An account on this device that can sign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExistingAccount {
    /// The account's public key, in hex.
    pub public_key_hex: String,
    /// The profile name, when MDK knows it.
    pub name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    #[error(transparent)]
    Mdk(#[from] AppError),
    #[error("the account was not published to its relays within {0:?}")]
    PublicationTimeout(Duration),
    #[error("setting up the account stopped and needs recovery")]
    RecoveryRequired,
}

/// The messaging side of the core: MDK's runtime over the stores in one data folder.
pub struct Messaging {
    runtime: MarmotAppRuntime,
    relays: Vec<String>,
}

impl Messaging {
    /// Opens the stores in the data folder and starts the runtime. Account keys live in the OS
    /// keychain. Must run inside a Tokio runtime, which MDK's relay tasks use.
    pub async fn open(options: MessagingOptions) -> Result<Self, MessagingError> {
        let home = AccountHome::open_with_keychain(&options.data_dir, options.keychain_service)
            .map_err(AppError::from)?;
        let config = MarmotAppConfig::default()
            .with_allow_loopback_relay_endpoints(options.allow_loopback_relays);
        let app = MarmotApp::try_with_relays_and_account_home_and_config(
            &options.data_dir,
            options.relays.clone(),
            home,
            config,
        )?;
        let runtime = app.runtime();
        runtime.start().await?;
        Ok(Self {
            runtime,
            relays: options.relays,
        })
    }

    /// Creates an account with a new key and `name` as its profile name.
    ///
    /// Returns once the account exists on this device. Publishing its relay lists, profile and
    /// KeyPackage continues in the background; [`Self::wait_until_published`] waits for it.
    pub async fn create_account(&self, name: &str) -> Result<NewAccount, MessagingError> {
        let relays: Vec<TransportEndpoint> =
            self.relays.iter().cloned().map(TransportEndpoint).collect();
        let request = AccountSetupRequest {
            default_relays: relays.clone(),
            bootstrap_relays: relays,
            publish_missing_relay_lists: true,
            publish_initial_key_package: true,
            ..Default::default()
        };
        let profile = UserProfileMetadata {
            name: Some(name.to_owned()),
            display_name: Some(name.to_owned()),
            ..Default::default()
        };
        let created = self
            .runtime
            .create_identity_local_ready_with_initial_profile(request, profile)
            .await?;
        Ok(NewAccount {
            public_key_hex: created.account.account_id_hex,
        })
    }

    /// The accounts on this device that can sign, with their profile names when known.
    pub fn accounts(&self) -> Result<Vec<ExistingAccount>, MessagingError> {
        let accounts = self.runtime.accounts().managed_accounts()?;
        Ok(accounts
            .into_iter()
            .filter(|account| !account.signed_out && !account.unlinked)
            .map(|account| ExistingAccount {
                name: self
                    .runtime
                    .display_name_for_account_id(&account.account_id_hex),
                public_key_hex: account.account_id_hex,
            })
            .collect())
    }

    /// Signs the account out and removes it from this device: its databases and its key in the
    /// keychain. Telling relays and groups is best effort and does not block the local wipe.
    pub async fn delete_account(&self, public_key_hex: &str) -> Result<(), MessagingError> {
        self.runtime.sign_out_and_wipe(public_key_hex).await?;
        Ok(())
    }

    /// Waits until the relays have acknowledged the account's relay lists, profile and
    /// KeyPackage, so that others can invite it.
    pub async fn wait_until_published(
        &self,
        account: &NewAccount,
        timeout: Duration,
    ) -> Result<(), MessagingError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .runtime
                .account_setup_readiness(&account.public_key_hex)?
            {
                AccountSetupReadiness::NetworkReady => return Ok(()),
                AccountSetupReadiness::RecoveryRequired => {
                    return Err(MessagingError::RecoveryRequired);
                }
                _ if Instant::now() >= deadline => {
                    return Err(MessagingError::PublicationTimeout(timeout));
                }
                _ => tokio::time::sleep(READINESS_POLL).await,
            }
        }
    }

    /// Stops the runtime and closes every database. Call it before the process may be
    /// suspended; afterwards this value cannot be used.
    pub async fn close(&self) -> Result<(), MessagingError> {
        self.runtime.shutdown_and_close().await?;
        Ok(())
    }
}
