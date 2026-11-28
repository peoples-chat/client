//! Someone on another Marmot app built on MDK, as White Noise is (#120, #123).

use std::time::Duration;

use marmot_account::AccountHome;
use marmot_app::{
    AccountSetupReadiness, AccountSetupRequest, AppError, AppGroupRecord, MarmotApp,
    MarmotAppConfig, MarmotAppRuntime, MarmotDeployment, UserProfileMetadata,
};
use tempfile::TempDir;

use super::{WAIT, use_mock_keychain};

/// An account on a plain MDK app whose relay lists, profile and KeyPackage are on one relay.
pub struct OtherApp {
    _data: TempDir,
    _app: MarmotApp,
    pub runtime: MarmotAppRuntime,
    pub account_hex: String,
}

impl OtherApp {
    pub async fn start(relay: &str, keychain_service: &str, name: &str) -> Self {
        Self::start_with_media_servers(relay, keychain_service, name, Vec::new()).await
    }

    /// The chats it starts carry `media_servers`, as White Noise's do.
    pub async fn start_with_media_servers(
        relay: &str,
        keychain_service: &str,
        name: &str,
        media_servers: Vec<String>,
    ) -> Self {
        use_mock_keychain();
        let data = tempfile::tempdir().unwrap();
        let home = AccountHome::open_with_keychain(data.path(), keychain_service).unwrap();
        let app = MarmotApp::try_with_relays_and_account_home_and_config(
            data.path(),
            vec![relay.to_owned()],
            home,
            MarmotAppConfig::default()
                .with_deployment(MarmotDeployment {
                    new_group_media_endpoints: media_servers,
                    ..MarmotDeployment::neutral()
                })
                .with_allow_loopback_relay_endpoints(true)
                .with_allow_loopback_blob_endpoints(true),
        )
        .unwrap();
        let runtime = app.runtime();
        runtime.start().await.unwrap();
        let relays = vec![cgka_traits::TransportEndpoint(relay.to_owned())];
        let request = AccountSetupRequest {
            default_relays: relays.clone(),
            bootstrap_relays: relays,
            publish_missing_relay_lists: true,
            publish_initial_key_package: true,
            ..Default::default()
        };
        let profile = UserProfileMetadata {
            name: Some(name.to_owned()),
            ..Default::default()
        };
        let account_hex = runtime
            .create_identity_local_ready_with_initial_profile(request, profile)
            .await
            .unwrap()
            .account
            .account_id_hex;
        let deadline = tokio::time::Instant::now() + WAIT;
        while runtime.account_setup_readiness(&account_hex).unwrap()
            != AccountSetupReadiness::NetworkReady
        {
            assert!(tokio::time::Instant::now() < deadline, "not published");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Self {
            _data: data,
            _app: app,
            runtime,
            account_hex,
        }
    }

    /// Accepts the invitation to `chat_id` once it has arrived, and returns the chat's state.
    pub async fn accept(&self, chat_id: &str) -> AppGroupRecord {
        let group_id = cgka_traits::GroupId::new(hex::decode(chat_id).unwrap());
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match self
                .runtime
                .accept_group_invite(&self.account_hex, &group_id)
                .await
            {
                Ok(record) => return record,
                Err(AppError::AccountWorkerBusy | AppError::UnknownGroup(_))
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("couldn't accept the invitation: {error}"),
            }
        }
    }

    pub async fn close(self) {
        self.runtime.shutdown_and_close().await.unwrap();
    }
}
