//! Spike (#117): can the radio address record (#34) live in the chat's state instead of
//! travelling as a message? MDK 0.18 lets an app keep state of its own in a chat: an MLS app
//! component, in the group's `app_data_dictionary`, under an id from `0xf000` up. Only the chat's
//! members can read it, and changing it is a commit, not a message.
//!
//! These tests drive MDK directly, without the core, with two accounts in a direct chat on a test
//! relay, and look at what each side sees. Each account's record has a component of its own: the
//! account whose public key sorts first has [`RADIOS`], the other the id after it. One component
//! for both would lose a record in a race. Findings: `docs/spikes/117-chat-state.md`.

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use cgka_traits::engine::GroupEvent;
use cgka_traits::{GroupId, TransportEndpoint};
use marmot_account::AccountHome;
use marmot_app::{
    AccountSetupReadiness, AccountSetupRequest, AppError, MarmotApp, MarmotAppConfig,
    MarmotAppEvent, MarmotAppRuntime, TimelineMessageQuery, TimelinePagination,
    UserProfileMetadata,
};
use support::{TestRelay, WAIT, use_mock_keychain};
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio::time::Instant;

/// The spike's first component id for radio address records; the second is the next. Apps
/// allocate from `0xf000` up.
const RADIOS: u16 = 0xf910;

/// Records the size of real ones (`core/src/radio/address.rs`), with made-up keys.
const ADA_RECORD: &str = r#"{"v":0,"mesh":"meshcore","key":"3c5b0d6f0a8e4f1c9b7d2e6a4c8f0b1d3e5a7c9f1b3d5e7a9c1e3f5a7b9d1e3f","freq":869618,"bw":62500,"sf":8,"cr":8,"at":1763900000000}"#;
const BEA_RECORD: &str = r#"{"v":0,"mesh":"meshcore","key":"a1c3e5f7092b4d6f8a0c2e4f6a8b0d2f4a6c8e0a2c4e6f8a0b2d4f6a8c0e2a4c","freq":869618,"bw":62500,"sf":8,"cr":8,"at":1763900000000}"#;

/// The records in a chat's state: each account's under its public key, in hex.
type Radios = BTreeMap<String, String>;

/// One device with one account, straight on MDK.
struct Device {
    data: TempDir,
    service: String,
    relay: String,
    app: MarmotApp,
    runtime: MarmotAppRuntime,
    account: String,
    /// The other person in the device's one direct chat, once there is one.
    peer: String,
}

/// The component that holds `account`'s record in its direct chat with `peer`.
fn slot(account: &str, peer: &str) -> u16 {
    if account < peer { RADIOS } else { RADIOS + 1 }
}

async fn open(data: &Path, service: &str, relay: &str) -> (MarmotApp, MarmotAppRuntime) {
    let home = AccountHome::open_with_keychain(data, service).unwrap();
    let config = MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true);
    let app = MarmotApp::try_with_relays_and_account_home_and_config(
        data,
        vec![relay.to_owned()],
        home,
        config,
    )
    .unwrap();
    let runtime = app.runtime();
    runtime.start().await.unwrap();
    (app, runtime)
}

impl Device {
    /// A device with a new account named `name`, once the relay has its KeyPackage.
    async fn new(name: &str, relay: &str) -> Self {
        use_mock_keychain();
        let data = tempfile::tempdir().unwrap();
        let service = format!("chat.peoples.client.test-state-{name}");
        let (app, runtime) = open(data.path(), &service, relay).await;
        let relays = vec![TransportEndpoint(relay.to_owned())];
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
        let account = runtime
            .create_identity_local_ready_with_initial_profile(request, profile)
            .await
            .unwrap()
            .account
            .account_id_hex;
        let deadline = Instant::now() + WAIT;
        while !matches!(
            runtime.account_setup_readiness(&account).unwrap(),
            AccountSetupReadiness::NetworkReady
        ) {
            assert!(
                Instant::now() < deadline,
                "{name}'s account wasn't published"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Self {
            data,
            service,
            relay: relay.to_owned(),
            app,
            runtime,
            account,
            peer: String::new(),
        }
    }

    /// Stops MDK and starts it again on the same folder and keys.
    async fn restart(&mut self) {
        self.runtime.shutdown_and_close().await.unwrap();
        let (app, runtime) = open(self.data.path(), &self.service, &self.relay).await;
        self.app = app;
        self.runtime = runtime;
    }

    /// The record in the component `component` of the chat's state, as this device has it now.
    async fn component(&self, chat: &GroupId, component: u16) -> Option<String> {
        let state = self
            .runtime
            .group_app_component(&self.account, chat, component)
            .await
            .unwrap();
        state.map(|bytes| String::from_utf8(bytes).unwrap())
    }

    /// Both people's records in the chat's state, as this device has it now.
    async fn radios(&self, chat: &GroupId) -> Radios {
        let mut radios = Radios::new();
        for account in [&self.account, &self.peer] {
            let component = slot(
                account,
                if account == &self.account {
                    &self.peer
                } else {
                    &self.account
                },
            );
            if let Some(record) = self.component(chat, component).await {
                radios.insert(account.clone(), record);
            }
        }
        radios
    }

    /// Puts `record` in the chat's state as this account's, unless it is there already. Returns
    /// whether it wrote.
    async fn share(&self, chat: &GroupId, record: &str) -> Result<bool, AppError> {
        let component = slot(&self.account, &self.peer);
        if self.component(chat, component).await.as_deref() == Some(record) {
            return Ok(false);
        }
        self.runtime
            .update_app_component(&self.account, chat, component, record.as_bytes().to_vec())
            .await?;
        Ok(true)
    }

    async fn epoch(&self, chat: &GroupId) -> u64 {
        self.runtime
            .group_mls_state(&self.account, chat)
            .await
            .unwrap()
            .epoch
    }

    /// What the chat looks like on this device: what an MDK app, White Noise among them, draws.
    fn view(&self, chat: &GroupId) -> View {
        let page = self
            .runtime
            .timeline_messages_with_query(
                &self.account,
                TimelineMessageQuery {
                    group_id_hex: Some(chat.to_string()),
                    pagination: TimelinePagination {
                        limit: Some(200),
                        ..TimelinePagination::default()
                    },
                    ..TimelineMessageQuery::default()
                },
            )
            .unwrap();
        let row = self
            .runtime
            .chat_list_row(&self.account, &chat.to_string())
            .unwrap()
            .expect("the chat is in the chat list");
        View {
            rows: page
                .messages
                .iter()
                .map(|message| (message.kind, message.plaintext.clone()))
                .collect(),
            unread: row.unread_count,
            activity: row.activity_sort_at,
            last_message: row.last_message.map(|message| message.message_id_hex),
        }
    }

    async fn send(&self, chat: &GroupId, text: &str) {
        self.runtime
            .submit_text(
                &self.account,
                chat,
                text.to_owned(),
                format!("spike-{text}"),
            )
            .await
            .unwrap();
    }

    /// Waits until this device's copy of the chat holds a text message `text`.
    async fn receives(&self, chat: &GroupId, text: &str) {
        let deadline = Instant::now() + WAIT;
        while !self
            .view(chat)
            .rows
            .iter()
            .any(|(kind, plaintext)| *kind == 9 && plaintext == text)
        {
            assert!(Instant::now() < deadline, "{text:?} didn't arrive");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Waits until this device's copy of the chat holds `want`, and says how long that took.
    async fn comes_to_hold(&self, chat: &GroupId, want: &Radios) -> Duration {
        let start = Instant::now();
        while self.radios(chat).await != *want {
            assert!(start.elapsed() < WAIT, "the records never came: {want:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        start.elapsed()
    }

    async fn close(self) {
        self.runtime.shutdown_and_close().await.unwrap();
    }
}

#[derive(Debug, PartialEq, Eq)]
struct View {
    /// Each row's kind and text, oldest first.
    rows: Vec<(u64, String)>,
    unread: u64,
    /// What orders the chat list.
    activity: u64,
    last_message: Option<String>,
}

/// Ada starts a direct chat with Bea, and Bea accepts it. Returns the chat's id.
async fn direct_chat(ada: &mut Device, bea: &mut Device) -> GroupId {
    ada.peer = bea.account.clone();
    bea.peer = ada.account.clone();
    let chat = ada
        .runtime
        .start_direct_conversation(&ada.account, &bea.account)
        .await
        .unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        match bea.runtime.accept_group_invite(&bea.account, &chat).await {
            Ok(_) => break,
            Err(AppError::AccountWorkerBusy | AppError::UnknownGroup(_))
                if Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("Bea couldn't accept: {error}"),
        }
    }
    chat
}

/// Waits until both copies of the chat are in the same epoch and say nothing is pending.
async fn settled(ada: &Device, bea: &Device, chat: &GroupId) -> u64 {
    let deadline = Instant::now() + WAIT;
    loop {
        let (a, b) = (
            ada.runtime
                .group_mls_state(&ada.account, chat)
                .await
                .unwrap(),
            bea.runtime
                .group_mls_state(&bea.account, chat)
                .await
                .unwrap(),
        );
        if a.epoch == b.epoch
            && a.lifecycle_state == marmot_app::AppGroupLifecycleState::Stable
            && b.lifecycle_state == marmot_app::AppGroupLifecycleState::Stable
        {
            return a.epoch;
        }
        assert!(Instant::now() < deadline, "the chat didn't settle");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Whether the device's copy of the chat is settled: no change of its own waiting.
async fn stable(device: &Device, chat: &GroupId) -> bool {
    device
        .runtime
        .group_mls_state(&device.account, chat)
        .await
        .unwrap()
        .lifecycle_state
        == marmot_app::AppGroupLifecycleState::Stable
}

/// Each event's name, as MDK names it, and for a group event the engine's, for the findings.
fn names(events: &[MarmotAppEvent]) -> Vec<String> {
    let name =
        |debug: String| debug[..debug.find([' ', '(', '{']).unwrap_or(debug.len())].to_owned();
    events
        .iter()
        .map(|event| match event {
            MarmotAppEvent::GroupEvent(group) => {
                format!("GroupEvent({})", name(format!("{:?}", group.event)))
            }
            other => name(format!("{other:?}")),
        })
        .collect()
}

/// Whether MDK told the device that the chat moved to a new epoch: the sign that its state may
/// have changed.
fn epoch_changed(events: &[MarmotAppEvent], chat: &GroupId) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            MarmotAppEvent::GroupEvent(group)
                if matches!(&group.event, GroupEvent::EpochChanged { group_id, .. } if group_id == chat)
        )
    })
}

fn drain(events: &mut broadcast::Receiver<MarmotAppEvent>) -> Vec<MarmotAppEvent> {
    let mut drained = Vec::new();
    while let Ok(event) = events.try_recv() {
        drained.push(event);
    }
    drained
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_in_the_chat_state_reach_the_contact_and_leave_no_trace() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let mut ada = Device::new("ada", &url).await;
    let mut bea = Device::new("bea", &url).await;
    let chat = direct_chat(&mut ada, &mut bea).await;
    ada.send(&chat, "Hej Bea").await;
    bea.receives(&chat, "Hej Bea").await;
    let start_epoch = settled(&ada, &bea, &chat).await;
    let ada_before = ada.view(&chat);
    let bea_before = bea.view(&chat);
    println!("before: epoch {start_epoch}, Bea's chat {bea_before:?}");
    assert_eq!(bea.radios(&chat).await, Radios::new());

    // Ada, who started the chat, writes her record. Bea's copy comes to hold it.
    let mut bea_events = bea.runtime.subscribe();
    let written = Instant::now();
    assert!(ada.share(&chat, ADA_RECORD).await.unwrap());
    let write_took = written.elapsed();
    let ada_only = Radios::from([(ada.account.clone(), ADA_RECORD.to_owned())]);
    assert_eq!(ada.radios(&chat).await, ada_only);
    bea.comes_to_hold(&chat, &ada_only).await;
    let arrived_after = written.elapsed();
    let events = drain(&mut bea_events);
    println!(
        "Ada's write took {write_took:?}; Bea held it {arrived_after:?} after the write began"
    );
    println!("Bea's events: {:?}", names(&events));
    assert!(
        epoch_changed(&events, &chat),
        "MDK didn't tell Bea's app that the chat changed"
    );

    // Neither chat shows a thing: no row, no unread count, no move up the chat list.
    let epoch = settled(&ada, &bea, &chat).await;
    assert_eq!(epoch, start_epoch + 1, "the write is one commit");
    assert_eq!(bea.view(&chat), bea_before);
    assert_eq!(ada.view(&chat), ada_before);

    // Bea, who was invited, can write too: MDK made both of them admins of the chat.
    let mut ada_events = ada.runtime.subscribe();
    assert!(bea.share(&chat, BEA_RECORD).await.unwrap());
    let both = Radios::from([
        (ada.account.clone(), ADA_RECORD.to_owned()),
        (bea.account.clone(), BEA_RECORD.to_owned()),
    ]);
    ada.comes_to_hold(&chat, &both).await;
    assert!(epoch_changed(&drain(&mut ada_events), &chat));
    // Writing the same record again changes nothing and sends nothing.
    assert!(!bea.share(&chat, BEA_RECORD).await.unwrap());
    let epoch = settled(&ada, &bea, &chat).await;
    assert_eq!(epoch, start_epoch + 2);
    assert_eq!(bea.view(&chat), bea_before);
    assert_eq!(ada.view(&chat), ada_before);

    // The chat goes on across the new epochs.
    bea.send(&chat, "Hej Ada").await;
    ada.receives(&chat, "Hej Ada").await;

    // The records stay across a restart.
    bea.restart().await;
    assert_eq!(bea.radios(&chat).await, both);

    // Ada changes her record without a relay. What does MDK make of it?
    let mut ada_events = ada.runtime.subscribe();
    relay.block();
    let newer = ADA_RECORD.replace("1763900000000", "1763900060000");
    let offline = tokio::time::timeout(Duration::from_secs(90), ada.share(&chat, &newer)).await;
    let state = ada
        .runtime
        .group_mls_state(&ada.account, &chat)
        .await
        .unwrap();
    println!(
        "an offline write gave {offline:?}; Ada's copy is then at epoch {}, {:?}, with her {} record",
        state.epoch,
        state.lifecycle_state,
        if ada
            .component(&chat, slot(&ada.account, &bea.account))
            .await
            .as_deref()
            == Some(&newer)
        {
            "new"
        } else {
            "old"
        }
    );
    tokio::time::sleep(Duration::from_secs(10)).await;
    let state = ada
        .runtime
        .group_mls_state(&ada.account, &chat)
        .await
        .unwrap();
    println!(
        "10 s later, still offline: epoch {}, {:?}; Ada's events {:?}",
        state.epoch,
        state.lifecycle_state,
        names(&drain(&mut ada_events))
    );
    relay.unblock();

    // Once the relay is back, both copies come to hold the newer record: by MDK alone, or after
    // Ada's app writes it again whenever her settled copy lacks it.
    let newer_both = Radios::from([
        (ada.account.clone(), newer.clone()),
        (bea.account.clone(), BEA_RECORD.to_owned()),
    ]);
    let back = Instant::now();
    let mut rewrites = 0;
    while ada.radios(&chat).await != newer_both || bea.radios(&chat).await != newer_both {
        assert!(back.elapsed() < WAIT, "the newer record never reached Bea");
        if stable(&ada, &chat).await && ada.share(&chat, &newer).await.unwrap() {
            rewrites += 1;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let epoch = settled(&ada, &bea, &chat).await;
    println!(
        "after the relay came back: both held the newer record {:?} later, after {rewrites} writes by the app; epoch {epoch}; Ada's events {:?}",
        back.elapsed(),
        names(&drain(&mut ada_events))
    );
    assert_eq!(
        bea.view(&chat).rows,
        ada_before
            .rows
            .iter()
            .cloned()
            .chain([(9, "Hej Ada".to_owned())])
            .collect::<Vec<_>>()
    );

    ada.close().await;
    bea.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_sides_write_at_once_and_each_keeps_its_record() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let mut ada = Device::new("ada-race", &url).await;
    let mut bea = Device::new("bea-race", &url).await;
    let chat = direct_chat(&mut ada, &mut bea).await;
    let start_epoch = settled(&ada, &bea, &chat).await;
    let ada_before = ada.view(&chat);
    let bea_before = bea.view(&chat);

    // Both write without a relay, as when both pair a radio during an outage, and the relay comes
    // back. Each change was made in the same epoch without the other's: a true race. (Online, MDK
    // fetches what the relay has before it sends, so the second change goes after the first.)
    let mut ada_events = ada.runtime.subscribe();
    let mut bea_events = bea.runtime.subscribe();
    relay.block();
    let (a, b) = tokio::join!(ada.share(&chat, ADA_RECORD), bea.share(&chat, BEA_RECORD));
    println!("the writes: Ada {a:?}, Bea {b:?}");
    assert!(!stable(&ada, &chat).await && !stable(&bea, &chat).await);
    relay.unblock();
    let raced = Instant::now();

    // First let the race settle without writing again, to see what MDK makes of it. A change it
    // sends again may still be on its way after the chat first settles.
    settled(&ada, &bea, &chat).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let epoch = settled(&ada, &bea, &chat).await;
    let mut superseded = Vec::new();
    let mut seen = Vec::new();
    for (who, events) in [("Ada", &mut ada_events), ("Bea", &mut bea_events)] {
        let events = drain(events);
        seen.push(format!("{who}: {:?}", names(&events)));
        for event in events {
            if let MarmotAppEvent::GroupChangeSuperseded {
                outcome, reason, ..
            } = event
            {
                superseded.push(format!("{who}: {} ({reason})", outcome.as_str()));
            }
        }
    }
    println!(
        "after the race, with no writes again, {:?} later: epoch {start_epoch} → {epoch}, Ada holds {:?}, Bea {:?}; superseded: {superseded:?}; events {seen:?}",
        raced.elapsed(),
        ada.radios(&chat).await.len(),
        bea.radios(&chat).await.len()
    );

    // Then each keeps its own record in the chat's state, as the app would: whenever its settled
    // copy lacks it, it writes it again.
    let both = Radios::from([
        (ada.account.clone(), ADA_RECORD.to_owned()),
        (bea.account.clone(), BEA_RECORD.to_owned()),
    ]);
    let mut rewrites = 0;
    while ada.radios(&chat).await != both || bea.radios(&chat).await != both {
        assert!(raced.elapsed() < WAIT, "the records never met");
        for (device, record) in [(&ada, ADA_RECORD), (&bea, BEA_RECORD)] {
            if stable(device, &chat).await && device.share(&chat, record).await.unwrap() {
                rewrites += 1;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    settled(&ada, &bea, &chat).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let epoch = settled(&ada, &bea, &chat).await;
    println!(
        "both held both {:?} after the race, after {rewrites} writes by the apps; epoch {epoch}",
        raced.elapsed()
    );
    assert_eq!(ada.radios(&chat).await, both);
    assert_eq!(bea.radios(&chat).await, both);
    assert_eq!(bea.view(&chat), bea_before);
    assert_eq!(ada.view(&chat), ada_before);

    ada.close().await;
    bea.close().await;
}

/// Why each record has a component of its own: with both records in one, the same race loses one.
/// The change that lost replaced the same component, so MDK drops it, and the app would have to
/// merge and write again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_component_for_both_records_loses_one_in_a_race() {
    /// The records of both, each under its account's public key, as JSON.
    const SHARED: u16 = 0xf90f;
    async fn shared(device: &Device, chat: &GroupId) -> Radios {
        let state = device
            .runtime
            .group_app_component(&device.account, chat, SHARED)
            .await
            .unwrap();
        state
            .map(|bytes| serde_json::from_slice(&bytes).unwrap())
            .unwrap_or_default()
    }
    async fn share_in_one(device: &Device, chat: &GroupId, record: &str) {
        let mut radios = shared(device, chat).await;
        radios.insert(device.account.clone(), record.to_owned());
        let data = serde_json::to_vec(&radios).unwrap();
        device
            .runtime
            .update_app_component(&device.account, chat, SHARED, data)
            .await
            .unwrap();
    }

    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let mut ada = Device::new("ada-one", &url).await;
    let mut bea = Device::new("bea-one", &url).await;
    let chat = direct_chat(&mut ada, &mut bea).await;
    settled(&ada, &bea, &chat).await;

    let mut ada_events = ada.runtime.subscribe();
    let mut bea_events = bea.runtime.subscribe();
    relay.block();
    tokio::join!(
        share_in_one(&ada, &chat, ADA_RECORD),
        share_in_one(&bea, &chat, BEA_RECORD)
    );
    relay.unblock();
    settled(&ada, &bea, &chat).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    settled(&ada, &bea, &chat).await;

    let mut superseded = Vec::new();
    for (who, events) in [("Ada", &mut ada_events), ("Bea", &mut bea_events)] {
        for event in drain(events) {
            if let MarmotAppEvent::GroupChangeSuperseded { outcome, .. } = event {
                superseded.push(format!("{who}: {}", outcome.as_str()));
            }
        }
    }
    let (a, b) = (shared(&ada, &chat).await, shared(&bea, &chat).await);
    println!("one component: Ada holds {a:?}, Bea {b:?}; superseded: {superseded:?}");
    assert_eq!(a, b);
    assert_eq!(a.len(), 1, "one record should have been lost");

    ada.close().await;
    bea.close().await;
}

/// For the check against White Noise's MDK (0.17), whose command-line client stands in for its
/// apps: writes the record in a chat with an account on another client, on a relay outside this
/// test, between two messages. Run the relay with `cargo run -p peoples-chat-core --example
/// local_relay -- 7790`, then
/// `PEER=<hex> RELAY=ws://127.0.0.1:7790 cargo test -p peoples-chat-core --test chat_state_spike
/// -- --ignored --nocapture`. It waits `PAUSE` seconds (30) after each step, so that the other
/// client can sync in between, and holds the chat for `HOLD` seconds (60) at the end, then stops;
/// its account lives only in a temporary folder.
///
/// The other client must run MDK live, as the apps do: `wn daemon start`, not `wn sync`, which
/// holds a commit and never applies it. For `wn` to stay on the local relay, build it with
/// `MarmotAppConfig::default()` in place of `MarmotAppConfig::white_noise()` and set
/// `WN_ALLOW_LOOPBACK_RELAYS=1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a relay and another client"]
async fn against_another_client() {
    let relay = std::env::var("RELAY").expect("RELAY");
    let peer = std::env::var("PEER").expect("PEER");
    let seconds = |name: &str, default: u64| {
        std::env::var(name).map_or(default, |value| value.parse().unwrap())
    };
    let (pause, hold) = (seconds("PAUSE", 30), seconds("HOLD", 60));
    let step = |what: String| async move {
        println!("step: {what}; waiting {pause} s");
        tokio::time::sleep(Duration::from_secs(pause)).await;
    };
    let mut ada = Device::new("ada-other", &relay).await;
    ada.peer = peer.clone();
    let chat = ada
        .runtime
        .start_direct_conversation(&ada.account, &peer)
        .await
        .unwrap();
    step(format!("chat {chat} started")).await;
    ada.send(&chat, "Before the record").await;
    step("sent \"Before the record\"".to_owned()).await;
    let epoch = ada.epoch(&chat).await;
    assert!(ada.share(&chat, ADA_RECORD).await.unwrap());
    step(format!(
        "record written: epoch {epoch} → {}",
        ada.epoch(&chat).await
    ))
    .await;
    ada.send(&chat, "After the record").await;
    println!("step: sent \"After the record\"; holding the chat for {hold} s");
    tokio::time::sleep(Duration::from_secs(hold)).await;
    println!(
        "Ada's chat at the end: epoch {}, {:?}",
        ada.epoch(&chat).await,
        ada.view(&chat)
    );
    ada.close().await;
}
