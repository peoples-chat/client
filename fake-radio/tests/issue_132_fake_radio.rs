//! Issue #132: meshcore-rs, unchanged, against fake companion radios over TCP.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use meshcore_rs::MeshCore;
use meshcore_rs::commands::Destination;
use meshcore_rs::events::{EventPayload, EventType, MeshCoreEvent};
use peoples_chat_fake_radio::{Config, Contact, FakeRadio, Fixture, TableFull, link, pair};
use tokio::sync::broadcast::Receiver;

/// Long enough for a slow machine; only a failure waits this long.
const WAIT: Duration = Duration::from_secs(10);
/// How long to watch for something that must not happen.
const QUIET: Duration = Duration::from_millis(500);

/// A meshcore-rs client on a fake radio, and its events from the start.
struct App {
    link: MeshCore,
    events: Receiver<MeshCoreEvent>,
}

impl App {
    async fn connect(radio: &FakeRadio) -> Self {
        let addr = radio.addr();
        let link = MeshCore::tcp(&addr.ip().to_string(), addr.port())
            .await
            .expect("meshcore-rs didn't connect");
        let events = link.dispatcher().receiver();
        let app = Self { link, events };
        // As companion apps greet a radio.
        let commands = app.link.commands().lock().await;
        commands.send_appstart().await.unwrap();
        commands.send_device_query().await.unwrap();
        drop(commands);
        app
    }

    /// Sends text to another radio, and returns the code its confirmation will carry.
    async fn send(&self, to: &FakeRadio, text: &str) -> [u8; 4] {
        let to = Destination::Bytes(to.public_key().to_vec());
        let commands = self.link.commands().lock().await;
        let sent = commands.send_msg(to, text, None).await.unwrap();
        sent.expected_ack
    }

    /// The next event of a kind, if one comes within `wait`.
    async fn next(&mut self, kind: EventType, wait: Duration) -> Option<EventPayload> {
        tokio::time::timeout(wait, async {
            loop {
                let event = self.events.recv().await.expect("events stopped");
                if event.event_type == kind {
                    return event.payload;
                }
            }
        })
        .await
        .ok()
    }

    /// Collects everything waiting on the radio, and returns the texts.
    async fn collect(&mut self) -> Vec<String> {
        let mut texts = Vec::new();
        loop {
            let more = {
                let commands = self.link.commands().lock().await;
                commands.get_msg().await.unwrap().is_some()
            };
            if !more {
                return texts;
            }
            match self.next(EventType::ContactMsgRecv, WAIT).await {
                Some(EventPayload::ContactMessage(message)) => texts.push(message.text),
                other => panic!("expected a message, got {other:?}"),
            }
        }
    }

    async fn confirmation(&mut self, wait: Duration) -> Option<[u8; 4]> {
        match self.next(EventType::Ack, wait).await? {
            EventPayload::Ack { tag } => Some(tag),
            other => panic!("expected a confirmation, got {other:?}"),
        }
    }
}

fn named(name: &str) -> Config {
    Config {
        name: name.to_owned(),
        ..Config::default()
    }
}

fn contact(byte: u8) -> Contact {
    Contact {
        public_key: [byte; 32],
        name: format!("Contact {byte}"),
        kind: 1,
        last_advert: 1_795_300_000,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn meshcore_rs_reads_identity_device_and_contacts() {
    let (ada, bea) = pair(named("Ada"), named("Bea")).await.unwrap();
    let app = App::connect(&ada).await;
    let commands = app.link.commands().lock().await;

    let me = commands.send_appstart().await.unwrap();
    assert_eq!(me.public_key, ada.public_key());
    assert_eq!(me.name, "Ada");
    assert_eq!((me.radio_freq, me.radio_bw), (869_618, 62_500));
    assert_eq!((me.sf, me.cr), (8, 8));

    let device = commands.send_device_query().await.unwrap();
    assert_eq!(device.version.as_deref(), Some("v1.18.0"));
    assert_eq!(device.model.as_deref(), Some("Fake radio"));

    let contacts = commands.get_contacts(0).await.unwrap();
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].public_key_hex(), hex::encode(bea.public_key()));
    assert_eq!(contacts[0].adv_name, "Bea");
    assert_eq!(contacts[0].contact_type, 1);
    // Asked for what changed since a later time, the radio has nothing.
    let later = commands.get_contacts(u32::MAX).await.unwrap();
    assert!(later.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_sent_on_one_of_a_pair_arrives_on_the_other_and_is_confirmed() {
    let (ada, bea) = pair(named("Ada"), named("Bea")).await.unwrap();
    let mut ada_app = App::connect(&ada).await;
    let mut bea_app = App::connect(&bea).await;

    let ack = ada_app.send(&bea, "Storm tonight").await;
    assert!(
        bea_app
            .next(EventType::MessagesWaiting, WAIT)
            .await
            .is_some()
    );
    let more = bea_app.link.commands().lock().await.get_msg().await;
    assert!(more.unwrap().is_some());
    match bea_app.next(EventType::ContactMsgRecv, WAIT).await {
        Some(EventPayload::ContactMessage(message)) => {
            assert_eq!(message.text, "Storm tonight");
            assert_eq!(message.sender_prefix[..], ada.public_key()[..6]);
            assert_eq!(message.snr, Some(7.25));
        }
        other => panic!("expected a message, got {other:?}"),
    }
    assert_eq!(ada_app.confirmation(WAIT).await, Some(ack));

    // And back.
    let ack = bea_app.send(&ada, "Coming at three").await;
    assert!(
        ada_app
            .next(EventType::MessagesWaiting, WAIT)
            .await
            .is_some()
    );
    assert_eq!(ada_app.collect().await, ["Coming at three"]);
    assert_eq!(bea_app.confirmation(WAIT).await, Some(ack));
}

#[tokio::test(flavor = "multi_thread")]
async fn loss_drops_messages_or_only_their_confirmations() {
    // Everything Ada puts on the air is lost.
    let lossy = Config {
        loss: 1.0,
        ..named("Ada")
    };
    let (ada, bea) = pair(lossy, named("Bea")).await.unwrap();
    let mut ada_app = App::connect(&ada).await;
    let mut bea_app = App::connect(&bea).await;
    ada_app.send(&bea, "Lost").await;
    assert!(
        bea_app
            .next(EventType::MessagesWaiting, QUIET)
            .await
            .is_none()
    );
    assert!(bea_app.collect().await.is_empty());
    assert_eq!(ada_app.confirmation(QUIET).await, None);

    // Everything Bea puts on the air is lost: the message arrives, its confirmation doesn't.
    let lossy = Config {
        loss: 1.0,
        ..named("Bea")
    };
    let (ada, bea) = pair(named("Ada"), lossy).await.unwrap();
    let mut ada_app = App::connect(&ada).await;
    let mut bea_app = App::connect(&bea).await;
    ada_app.send(&bea, "Unconfirmed").await;
    assert!(
        bea_app
            .next(EventType::MessagesWaiting, WAIT)
            .await
            .is_some()
    );
    assert_eq!(bea_app.collect().await, ["Unconfirmed"]);
    assert_eq!(ada_app.confirmation(QUIET).await, None);
}

/// Which of twelve messages get through a radio that loses half, with a seed.
async fn delivered_at_half_loss(seed: u64) -> Vec<String> {
    let lossy = Config {
        loss: 0.5,
        seed,
        ..named("Ada")
    };
    let (ada, bea) = pair(lossy, named("Bea")).await.unwrap();
    let ada_app = App::connect(&ada).await;
    let mut bea_app = App::connect(&bea).await;
    for number in 0..12 {
        ada_app.send(&bea, &number.to_string()).await;
    }
    tokio::time::sleep(QUIET).await;
    bea_app.collect().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_loss_rate_drops_some_and_the_seed_repeats_which() {
    let (first, again) = tokio::join!(delivered_at_half_loss(7), delivered_at_half_loss(7));
    assert!(!first.is_empty() && first.len() < 12, "{first:?}");
    assert_eq!(first, again);
}

#[tokio::test(flavor = "multi_thread")]
async fn delay_holds_back_messages_and_confirmations() {
    let delay = Duration::from_millis(700);
    let slow = |name| Config {
        delay,
        ..named(name)
    };
    let (ada, bea) = pair(slow("Ada"), slow("Bea")).await.unwrap();
    let mut ada_app = App::connect(&ada).await;
    let mut bea_app = App::connect(&bea).await;

    let started = Instant::now();
    let ack = ada_app.send(&bea, "Slow").await;
    assert!(
        bea_app
            .next(EventType::MessagesWaiting, WAIT)
            .await
            .is_some()
    );
    assert!(started.elapsed() >= delay);
    assert_eq!(ada_app.confirmation(WAIT).await, Some(ack));
    assert!(started.elapsed() >= delay * 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn duplication_delivers_a_message_twice() {
    let doubling = Config {
        duplication: 1.0,
        ..named("Ada")
    };
    let (ada, bea) = pair(doubling, named("Bea")).await.unwrap();
    let mut ada_app = App::connect(&ada).await;
    let mut bea_app = App::connect(&bea).await;

    let ack = ada_app.send(&bea, "Twice").await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(bea_app.collect().await, ["Twice", "Twice"]);
    // Both copies are confirmed, but the radio tells its app only once.
    assert_eq!(ada_app.confirmation(WAIT).await, Some(ack));
    assert_eq!(ada_app.confirmation(QUIET).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_contact_table_holds_only_its_size() {
    let small = Config {
        max_contacts: 2,
        ..named("Ada")
    };
    let ada = FakeRadio::start(small).await.unwrap();
    assert_eq!(ada.add_contact(contact(1)), Ok(()));
    assert_eq!(ada.add_contact(contact(2)), Ok(()));
    assert_eq!(ada.add_contact(contact(3)), Err(TableFull));
    // A contact it holds can still be updated.
    assert_eq!(ada.add_contact(contact(2)), Ok(()));

    let app = App::connect(&ada).await;
    {
        let commands = app.link.commands().lock().await;
        let device = commands.send_device_query().await.unwrap();
        assert_eq!(device.max_contacts, Some(2));
        assert_eq!(commands.get_contacts(0).await.unwrap().len(), 2);
    }

    // With no room for each other, a pair can't exchange messages.
    let bea = FakeRadio::start(named("Bea")).await.unwrap();
    assert_eq!(link(&ada, &bea), Err(TableFull));
    let mut bea_app = App::connect(&bea).await;
    bea_app.send(&ada, "Who is this?").await;
    assert_eq!(bea_app.confirmation(QUIET).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fixture_file_is_replayed() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/receive-and-send.frames"
    );
    let fixture = Fixture::load(path).unwrap();
    let radio = FakeRadio::replay(Config::default(), fixture).await.unwrap();

    let addr = radio.addr();
    let link = MeshCore::tcp(&addr.ip().to_string(), addr.port())
        .await
        .unwrap();
    let events = link.dispatcher().receiver();
    let mut app = App { link, events };

    let me = app.link.commands().lock().await.send_appstart().await;
    let me = me.unwrap();
    assert_eq!(me.name, "Recorded");
    assert_eq!(me.public_key, [0xad; 32]);
    assert_eq!((me.radio_freq, me.radio_bw), (869_618, 62_500));
    assert!(app.next(EventType::MessagesWaiting, WAIT).await.is_some());
    assert_eq!(app.collect().await, ["Coming at three"]);

    assert_eq!(radio.replay_remaining(), 0);
    assert!(radio.replay_mismatches().is_empty());
}

#[test]
fn a_malformed_fixture_names_its_line() {
    let error = "# fine\napp 0a\nradio zz\n".parse::<Fixture>().unwrap_err();
    assert!(error.to_string().contains("line 3"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn radios_started_together_get_their_own_ports() {
    let mut starting = tokio::task::JoinSet::new();
    for _ in 0..32 {
        starting.spawn(FakeRadio::start(Config::default()));
    }
    let radios: Vec<_> = starting
        .join_all()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    let ports: HashSet<u16> = radios.iter().map(|radio| radio.addr().port()).collect();
    assert_eq!(ports.len(), radios.len());

    // A port can also be chosen, and one that is taken is refused.
    let taken = Config {
        port: radios[0].addr().port(),
        ..Config::default()
    };
    assert!(FakeRadio::start(taken).await.is_err());
}
