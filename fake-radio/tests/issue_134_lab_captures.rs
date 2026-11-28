//! Issue #134: the frames of a `verify:radio` run on the lab's two radios load as fixtures, and
//! hold what the run did.

use peoples_chat_fake_radio::{Fixture, Sender};

#[test]
fn issue_134_the_lab_captures_load_as_fixtures() {
    for name in ["lab-r1.frames", "lab-r2.frames"] {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let fixture = Fixture::load(path).unwrap();
        let count = |sender, code| {
            fixture
                .frames
                .iter()
                .filter(|frame| frame.sender == sender && frame.payload[0] == code)
                .count()
        };
        // An advert and a message went out. The radio took the message (SENT), the other one
        // confirmed it, and a message came the other way.
        assert!(count(Sender::App, 7) >= 1, "{name}: no advert");
        assert!(count(Sender::App, 2) >= 1, "{name}: no message sent");
        assert!(count(Sender::Radio, 6) >= 1, "{name}: no SENT");
        assert!(count(Sender::Radio, 0x82) >= 1, "{name}: no confirmation");
        assert!(count(Sender::Radio, 16) >= 1, "{name}: no message received");
        // The Bluetooth PIN is zeroed.
        assert!(
            fixture
                .frames
                .iter()
                .filter(|frame| frame.sender == Sender::Radio && frame.payload[0] == 13)
                .all(|frame| frame.payload[4..8] == [0; 4])
        );
    }
}
