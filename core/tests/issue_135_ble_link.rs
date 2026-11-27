//! Issue #135: a link that ends silently, as a Bluetooth link does, is noticed and opened again
//! without restarting anything. A fake companion radio over TCP stands in for the radio: its
//! link can go quiet without closing. The tests of the bridge itself are in
//! `issue_135_ble_bridge.rs`.

mod support;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meshcore_rs::MeshCore;
use peoples_chat_core::Core;
use peoples_chat_core::interface::RadioConnection;
use peoples_chat_core::radio::{
    Mesh, MeshCoreRadio, Port, Radio, RadioContact, RadioError, RadioEvent, RadioInfo, RadioPorts,
    Sent,
};
use peoples_chat_fake_radio::{Config, FakeRadio, put_in_range};
use support::{Phone, TestRelay, until};
use tokio::sync::broadcast;

const BLE_PORT: &str = "ble:MeshCore-FAKE";
/// How often the tests check the link. The app checks a Bluetooth link every 10 seconds.
const WATCH_EVERY: Duration = Duration::from_millis(100);

/// The core's radio over a watched TCP link to a fake. The link lives as long as the radio.
struct Watched {
    radio: MeshCoreRadio,
    link: MeshCore,
}

impl Watched {
    async fn connect(fake: &FakeRadio) -> Result<Self, RadioError> {
        let addr = fake.addr();
        let link = MeshCore::tcp(&addr.ip().to_string(), addr.port())
            .await
            .map_err(|error| RadioError::Link(error.to_string()))?;
        link.set_default_timeout(Duration::from_millis(300)).await;
        let radio = MeshCoreRadio::over(link.commands().clone(), link.dispatcher()).await?;
        radio.watch_link(WATCH_EVERY);
        Ok(Self { radio, link })
    }
}

#[async_trait]
impl Radio for Watched {
    fn mesh(&self) -> Mesh {
        self.radio.mesh()
    }

    async fn info(&self) -> Result<RadioInfo, RadioError> {
        self.radio.info().await
    }

    async fn contacts(&self) -> Result<Vec<RadioContact>, RadioError> {
        self.radio.contacts().await
    }

    async fn send_text(&self, to: &str, text: &str) -> Result<Sent, RadioError> {
        self.radio.send_text(to, text).await
    }

    fn events(&self) -> broadcast::Receiver<RadioEvent> {
        self.radio.events()
    }

    async fn disconnect(&self) -> Result<(), RadioError> {
        self.radio.disconnect().await?;
        let _ = self.link.disconnect().await;
        Ok(())
    }
}

/// One fake radio where the app looks for Bluetooth radios.
struct FakeBluetooth {
    fake: Arc<FakeRadio>,
}

#[async_trait]
impl RadioPorts for FakeBluetooth {
    async fn list(&self) -> Result<Vec<Port>, RadioError> {
        Ok(vec![Port {
            path: BLE_PORT.to_owned(),
            description: Some("Bluetooth".to_owned()),
            serial_number: None,
        }])
    }

    async fn open(&self, _path: &str) -> Result<Arc<dyn Radio>, RadioError> {
        Ok(Arc::new(Watched::connect(&self.fake).await?))
    }
}

async fn next_event(events: &mut broadcast::Receiver<RadioEvent>) -> RadioEvent {
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("no radio event in time")
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_link_that_goes_quiet_is_reported_as_lost() {
    let fake = FakeRadio::start(Config::default()).await.unwrap();
    let radio = Watched::connect(&fake).await.unwrap();
    let mut events = radio.events();

    // While the radio answers, the checks say nothing.
    tokio::time::sleep(WATCH_EVERY * 5).await;
    assert!(events.try_recv().is_err());
    radio.info().await.unwrap();

    // Nothing closes; the radio just stops answering.
    fake.go_quiet();
    assert_eq!(next_event(&mut events).await, RadioEvent::Disconnected);
    assert!(matches!(
        radio.info().await.unwrap_err(),
        RadioError::NotConnected
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_radios_that_announce_themselves_become_contacts_and_keep_them_on_a_new_link() {
    let ada = FakeRadio::start(Config::default()).await.unwrap();
    let bea = FakeRadio::start(Config::default()).await.unwrap();
    put_in_range(&ada, &bea);
    let ada_radio = Watched::connect(&ada).await.unwrap();
    let bea_radio = Watched::connect(&bea).await.unwrap();
    assert!(ada_radio.contacts().await.unwrap().is_empty());

    ada_radio.radio.advertise().await.unwrap();
    bea_radio.radio.advertise().await.unwrap();
    let ada_key = hex::encode(ada.public_key());
    let bea_key = hex::encode(bea.public_key());
    let held = |contacts: Vec<RadioContact>| -> Vec<String> {
        contacts.into_iter().map(|c| c.public_key).collect()
    };
    assert_eq!(
        held(ada_radio.contacts().await.unwrap()),
        std::slice::from_ref(&bea_key)
    );
    assert_eq!(held(bea_radio.contacts().await.unwrap()), [ada_key]);

    // The link is lost without closing; the checks notice.
    let mut events = bea_radio.events();
    bea.go_quiet();
    assert_eq!(next_event(&mut events).await, RadioEvent::Disconnected);

    // A new link works, and the radio kept its contacts.
    let bea_again = Watched::connect(&bea).await.unwrap();
    let mut events = bea_again.events();
    ada_radio.send_text(&bea_key, "Still there?").await.unwrap();
    match next_event(&mut events).await {
        RadioEvent::TextReceived { text, .. } => assert_eq!(text, "Still there?"),
        other => panic!("expected a message, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_135_the_core_opens_a_radio_again_after_its_link_went_quiet() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let phone = Phone::new("ble-ada", &relay.url());
    let core: Core = phone.start().await;
    core.set_radio_reconnect_wait(Duration::from_millis(50));
    let fake = Arc::new(FakeRadio::start(Config::default()).await.unwrap());
    core.use_radio_ports(Arc::new(FakeBluetooth { fake: fake.clone() }));
    let connection = |core: &Core| core.snapshot().radio.paired.map(|radio| radio.connection);

    let status = core.pair_radio(BLE_PORT).await.unwrap();
    let paired = status.paired.unwrap();
    assert_eq!(paired.public_key, hex::encode(fake.public_key()));
    assert_eq!(paired.connection, RadioConnection::Connected);

    // The link goes quiet: nothing closes, and the radio just stops answering.
    let mut changes = core.subscribe();
    fake.go_quiet();
    until("the core noticing the lost link", || {
        (connection(&core) == Some(RadioConnection::Connecting)).then_some(())
    })
    .await;
    // The same core, still running, has the radio back.
    until("the radio back in use", || {
        (connection(&core) == Some(RadioConnection::Connected)).then_some(())
    })
    .await;
    assert!(changes.try_recv().is_ok(), "the interface was told");
    core.close().await.unwrap();
}
