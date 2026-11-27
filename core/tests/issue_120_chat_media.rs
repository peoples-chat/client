//! MDK's configuration (#120): chats the core starts carry media servers, so that a White Noise
//! user in them can attach a photo; our KeyPackages say which app made them; and people are also
//! looked up on White Noise's relays.
//!
//! The other person here is a plain MDK app, as White Noise is: it accepts the invitation and
//! attaches a photo the way White Noise does, through MDK's `upload_media`.

mod support;

use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::{Rgb, RgbImage};
use marmot_account::AccountHome;
use marmot_app::{
    AccountSetupReadiness, AccountSetupRequest, AppError, AppGroupRecord, MarmotApp,
    MarmotAppConfig, MarmotAppRuntime, MediaUploadAttachmentRequest, MediaUploadRequest,
    UserProfileMetadata,
};
use nostr_sdk::prelude::{Client, Filter, Kind, PublicKey, ReqTarget};
use peoples_chat_core::{CLIENT_NAME, Messaging, MessagingOptions};
use support::{TestBlossom, TestRelay, WAIT, options, use_mock_keychain};
use tempfile::TempDir;

/// MDK's addressable KeyPackage kind. The pinned nostr-sdk has no name for it.
const KEY_PACKAGE_KIND: u16 = 30443;

/// Someone on another Marmot app built on MDK, as White Noise is, with an account whose relay
/// lists, profile and KeyPackage are on `relay` only.
struct OtherApp {
    _data: TempDir,
    _app: MarmotApp,
    runtime: MarmotAppRuntime,
    account_hex: String,
}

impl OtherApp {
    async fn start(relay: &str, keychain_service: &str, name: &str) -> Self {
        use_mock_keychain();
        let data = tempfile::tempdir().unwrap();
        let home = AccountHome::open_with_keychain(data.path(), keychain_service).unwrap();
        let app = MarmotApp::try_with_relays_and_account_home_and_config(
            data.path(),
            vec![relay.to_owned()],
            home,
            MarmotAppConfig::default()
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
    async fn accept(&self, chat_id: &str) -> AppGroupRecord {
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

    async fn close(self) {
        self.runtime.shutdown_and_close().await.unwrap();
    }
}

async fn open_with_account(options: MessagingOptions, name: &str) -> (Messaging, String) {
    use_mock_keychain();
    let messaging = Messaging::open(options).await.unwrap();
    let account = messaging.create_account(name).await.unwrap();
    messaging
        .wait_until_published(&account, WAIT)
        .await
        .unwrap();
    (messaging, account.public_key_hex)
}

fn photo() -> Vec<u8> {
    let image = RgbImage::from_fn(64, 48, |x, y| Rgb([(x * 4) as u8, (y * 5) as u8, 90]));
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&image)
        .unwrap();
    jpeg
}

fn same_server(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chat_the_core_starts_carries_media_servers_and_the_other_person_can_attach_a_photo() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let media = TestBlossom::start().await;
    let data = tempfile::tempdir().unwrap();
    let (ada, ada_hex) = open_with_account(
        MessagingOptions {
            chat_media_servers: vec![media.url()],
            ..options(
                data.path(),
                &relay.url(),
                "chat.peoples.client.test-120-ada",
            )
        },
        "Ada Lund",
    )
    .await;
    let bea = OtherApp::start(&relay.url(), "chat.peoples.client.test-120-bea", "Bea Holm").await;

    // Ada starts the chat. Its state names the media server, for both of them.
    let chat_id = ada
        .start_direct_chat(&ada_hex, &bea.account_hex, &[])
        .await
        .unwrap();
    let servers = ada.media_servers_in_chat(&ada_hex, &chat_id).await.unwrap();
    assert_eq!(servers.len(), 1, "{servers:?}");
    assert!(same_server(&servers[0], &media.url()), "{servers:?}");

    let record = bea.accept(&chat_id).await;
    assert!(
        record.encrypted_media.required,
        "the chat has no encrypted-media component"
    );
    let endpoints: Vec<&str> = record
        .encrypted_media
        .default_blob_endpoints
        .iter()
        .map(|endpoint| endpoint.base_url.as_str())
        .collect();
    assert_eq!(endpoints.len(), 1, "{endpoints:?}");
    assert!(same_server(endpoints[0], &media.url()), "{endpoints:?}");

    // Bea attaches a photo, as White Noise does. It goes to the chat's server, encrypted.
    let photo = photo();
    let uploaded = bea
        .runtime
        .upload_media(
            &bea.account_hex,
            &record_group_id(&record),
            MediaUploadRequest {
                attachments: vec![MediaUploadAttachmentRequest {
                    file_name: "photo.jpg".to_owned(),
                    media_type: "image/jpeg".to_owned(),
                    plaintext: photo.clone(),
                    dim: Some("64x48".to_owned()),
                    thumbhash: None,
                }],
                caption: Some("Look".to_owned()),
                send: true,
                blossom_server: None,
                message_tags: Vec::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(uploaded.attachments.len(), 1);
    assert!(uploaded.sent.is_some(), "the photo wasn't sent");
    let blobs = media.blobs();
    assert_eq!(blobs.len(), 1);
    assert_ne!(blobs[0], photo, "the photo was uploaded unencrypted");

    bea.close().await;
    ada.close().await.unwrap();
}

fn record_group_id(record: &AppGroupRecord) -> cgka_traits::GroupId {
    cgka_traits::GroupId::new(hex::decode(&record.group_id_hex).unwrap())
}

/// A chat started without media servers can never carry photos: MDK founds it without the
/// component, and nothing can add it later. This is what every chat was before #120.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chat_started_without_media_servers_carries_none() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let data = tempfile::tempdir().unwrap();
    let (ada, ada_hex) = open_with_account(
        options(
            data.path(),
            &relay.url(),
            "chat.peoples.client.test-120-ada-none",
        ),
        "Ada Lund",
    )
    .await;
    let bea = OtherApp::start(
        &relay.url(),
        "chat.peoples.client.test-120-bea-none",
        "Bea Holm",
    )
    .await;
    let chat_id = ada
        .start_direct_chat(&ada_hex, &bea.account_hex, &[])
        .await
        .unwrap();
    assert!(
        ada.media_servers_in_chat(&ada_hex, &chat_id)
            .await
            .unwrap()
            .is_empty()
    );
    bea.close().await;
    ada.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn our_key_packages_say_they_come_from_peoples_chat() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let data = tempfile::tempdir().unwrap();
    let (ada, ada_hex) = open_with_account(
        options(data.path(), &url, "chat.peoples.client.test-120-client"),
        "Ada Lund",
    )
    .await;

    let client = Client::builder().build();
    client.add_relay(&url).await.unwrap();
    client.connect().await;
    let events = client
        .fetch_events(ReqTarget::single(
            &url,
            [Filter::new()
                .author(PublicKey::from_hex(&ada_hex).unwrap())
                .kind(Kind::from(KEY_PACKAGE_KIND))],
        ))
        .timeout(Duration::from_secs(5))
        .await
        .unwrap();
    assert!(!events.is_empty(), "no KeyPackage on the relay");
    for event in events.iter() {
        let clients: Vec<&[String]> = event
            .tags
            .iter()
            .map(|tag| tag.as_slice())
            .filter(|tag| tag.first().map(String::as_str) == Some("client"))
            .collect();
        assert_eq!(
            clients,
            [&["client".to_owned(), CLIENT_NAME.to_owned()][..]]
        );
    }
    assert_eq!(CLIENT_NAME, "peoples.chat");
    ada.close().await.unwrap();
}

/// Someone whose app published only on its own relays, as White Noise's users have, is found
/// there without an invite link's hints.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn people_are_also_looked_up_on_the_lookup_relays() {
    let _turn = support::take_turn().await;
    let ours = TestRelay::start().await;
    let theirs = TestRelay::start().await;
    let bea = OtherApp::start(
        &theirs.url(),
        "chat.peoples.client.test-120-bea-lookup",
        "Bea Holm",
    )
    .await;

    // Without the lookup relay, our relays alone don't know Bea.
    let cai_data = tempfile::tempdir().unwrap();
    let (cai, cai_hex) = open_with_account(
        options(
            cai_data.path(),
            &ours.url(),
            "chat.peoples.client.test-120-cai",
        ),
        "Cai Berg",
    )
    .await;
    let error = cai
        .start_direct_chat(&cai_hex, &bea.account_hex, &[])
        .await
        .unwrap_err();
    assert!(error.is_contact_unreachable(), "{error}");
    cai.close().await.unwrap();

    // With it, Ada finds Bea, and Bea gets the invitation.
    let ada_data = tempfile::tempdir().unwrap();
    let (ada, ada_hex) = open_with_account(
        MessagingOptions {
            lookup_relays: vec![theirs.url()],
            ..options(
                ada_data.path(),
                &ours.url(),
                "chat.peoples.client.test-120-ada-lookup",
            )
        },
        "Ada Lund",
    )
    .await;
    let chat_id = ada
        .start_direct_chat(&ada_hex, &bea.account_hex, &[])
        .await
        .unwrap();
    bea.accept(&chat_id).await;

    bea.close().await;
    ada.close().await.unwrap();
}
