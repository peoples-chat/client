//! Disappearing messages (#124): someone on White Noise for Android turns on the chat's timer,
//! and a message disappears from peoples.chat when its time is up, from the screen and from
//! disk.
//!
//! The other person here is a plain MDK app, as White Noise is: it accepts the chat, sets the
//! timer with MDK's `update_message_retention`, as an admin may, and sends text.

mod support;

use std::time::Duration;

use marmot_account::AccountHome;
use marmot_app::{
    AccountSetupReadiness, AccountSetupRequest, AppError, MarmotApp, MarmotAppConfig,
    MarmotAppRuntime, UserProfileMetadata,
};
use peoples_chat_core::interface::{CoreEvent, Message};
use peoples_chat_core::{Messaging, MessagingOptions};
use support::{Phone, TestRelay, WAIT, now_ms, options, until, use_mock_keychain};
use tempfile::TempDir;
use tokio::sync::broadcast;

/// The chat's timer: long enough for the message to arrive first, short for a test.
const TIMER_SECS: u64 = 8;

/// Someone on another Marmot app built on MDK, as White Noise for Android is.
struct WhiteNoise {
    _data: TempDir,
    _app: MarmotApp,
    runtime: MarmotAppRuntime,
    account_hex: String,
}

impl WhiteNoise {
    async fn start(relay: &str) -> Self {
        use_mock_keychain();
        let data = tempfile::tempdir().unwrap();
        let home = AccountHome::open_with_keychain(data.path(), "chat.peoples.client.test-124-wn")
            .unwrap();
        let app = MarmotApp::try_with_relays_and_account_home_and_config(
            data.path(),
            vec![relay.to_owned()],
            home,
            MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true),
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
            name: Some("Wren Noise".to_owned()),
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

    fn group(chat_id: &str) -> cgka_traits::GroupId {
        cgka_traits::GroupId::new(hex::decode(chat_id).unwrap())
    }

    async fn accept(&self, chat_id: &str) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match self
                .runtime
                .accept_group_invite(&self.account_hex, &Self::group(chat_id))
                .await
            {
                Ok(_) => return,
                Err(AppError::AccountWorkerBusy | AppError::UnknownGroup(_))
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("couldn't accept the invitation: {error}"),
            }
        }
    }

    /// Sets the chat's timer, as White Noise for Android's chat details do.
    async fn set_timer(&self, chat_id: &str, secs: u64) {
        self.runtime
            .update_message_retention(&self.account_hex, &Self::group(chat_id), secs)
            .await
            .unwrap();
    }

    async fn send(&self, chat_id: &str, text: &str) -> String {
        self.runtime
            .submit_text(
                &self.account_hex,
                &Self::group(chat_id),
                text.to_owned(),
                format!("wn-{}", now_ms()),
            )
            .await
            .unwrap()
            .message_id_hex
    }

    async fn close(self) {
        self.runtime.shutdown_and_close().await.unwrap();
    }
}

/// The message `id` in the chat, as the snapshot has it.
fn shown(core: &peoples_chat_core::Core, chat_id: &str, id: &str) -> Option<Message> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)?
        .messages
        .into_iter()
        .find(|message| message.id == id)
}

/// Waits for `messageRemoved` for `id`, skipping other events.
async fn until_removed(events: &mut broadcast::Receiver<CoreEvent>, chat_id: &str, id: &str) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("the message didn't disappear in time")
            .unwrap();
        if let CoreEvent::MessageRemoved {
            chat_id: from,
            message_id,
        } = event
            && from == chat_id
            && message_id == id
        {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_124_a_message_disappears_when_its_time_is_up_from_the_screen_and_from_disk() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let phone = Phone::new("disappearing-bea", &url);
    let bea = phone.start().await;
    let bea_hex = bea.create_account("Bea Holm").await.unwrap().public_key;
    bea.wait_until_published(WAIT).await.unwrap();
    let wn = WhiteNoise::start(&url).await;

    // Bea adds the White Noise user by key, and they accept.
    let chat_id = bea.add_contact(&wn.account_hex).await.unwrap().id;
    wn.accept(&chat_id).await;
    let before = wn.send(&chat_id, "Before the timer").await;
    until("the message from before the timer", || {
        shown(&bea, &chat_id, &before)
    })
    .await;
    let chat = bea.snapshot().chats.into_iter().next().unwrap();
    assert_eq!(chat.disappear_after, None);

    // They turn on disappearing messages. Bea's chat says so, and after how long.
    let mut events = bea.subscribe();
    wn.set_timer(&chat_id, TIMER_SECS).await;
    until("the chat's timer", || {
        let chat = bea.snapshot().chats.into_iter().next()?;
        (chat.disappear_after == Some(TIMER_SECS)).then_some(())
    })
    .await;

    // A message sent now carries when it disappears, by the sender's clock.
    let sent = wn.send(&chat_id, "This disappears").await;
    let received = until("the disappearing message", || shown(&bea, &chat_id, &sent)).await;
    let expires_at = received
        .expires_at
        .expect("the message says when it disappears");
    assert_eq!(expires_at, received.sent_at + TIMER_SECS * 1000);
    // Bea's own message in the chat disappears too.
    let own = bea.send_message(&chat_id, "Mine too").await.unwrap();
    let own_expires = own
        .expires_at
        .expect("Bea's message says when it disappears");
    assert_eq!(own_expires, own.sent_at + TIMER_SECS * 1000);

    // Bea sees the chat. MDK deletes only what is read.
    bea.mark_read(&chat_id).await.unwrap();

    // Gone from the screen when its time is up, not before.
    until_removed(&mut events, &chat_id, &sent).await;
    assert!(
        now_ms() + 1000 >= expires_at,
        "it disappeared {} ms early",
        expires_at - now_ms()
    );
    until("Bea's own message to disappear", || {
        shown(&bea, &chat_id, &own.id).is_none().then_some(())
    })
    .await;
    assert!(shown(&bea, &chat_id, &sent).is_none());
    // What came before the timer stays.
    assert!(shown(&bea, &chat_id, &before).is_some());

    // MDK deletes them once 5 seconds of clock skew have passed; the core asks soon after.
    tokio::time::sleep(Duration::from_secs(9)).await;
    bea.close().await.unwrap();

    // From disk: MDK's database no longer has them.
    let options = MessagingOptions {
        ..options(phone.data_dir(), &url, phone.keychain_service())
    };
    let messaging = Messaging::open(options).await.unwrap();
    let kept: Vec<String> = messaging
        .chat_messages(&bea_hex, &chat_id)
        .unwrap()
        .into_iter()
        .map(|message| message.id)
        .collect();
    assert!(kept.contains(&before), "{kept:?}");
    assert!(!kept.contains(&sent), "{kept:?}");
    assert!(!kept.contains(&own.id), "{kept:?}");
    assert!(messaging.chat_message(&bea_hex, &chat_id, &sent).is_none());
    // Bea's app marked what she saw as read, on this device only.
    assert!(!messaging.has_unread(&bea_hex, &chat_id).unwrap());
    messaging.close().await.unwrap();

    // And after a restart the chat still says its messages disappear, without them.
    let bea = phone.start().await;
    let chat = bea.snapshot().chats.into_iter().next().unwrap();
    assert_eq!(chat.disappear_after, Some(TIMER_SECS));
    let ids: Vec<&str> = chat.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, [before.as_str()]);

    // Turned off again, new messages stay.
    wn.set_timer(&chat_id, 0).await;
    until("the timer to be off", || {
        let chat = bea.snapshot().chats.into_iter().next()?;
        chat.disappear_after.is_none().then_some(())
    })
    .await;
    let after = wn.send(&chat_id, "This stays").await;
    let stays = until("the message after the timer", || {
        shown(&bea, &chat_id, &after)
    })
    .await;
    assert_eq!(stays.expires_at, None);

    bea.close().await.unwrap();
    wn.close().await;
}
