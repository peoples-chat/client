//! Choosing the transport for each message (#45), by the spec's three rules:
//!
//! 1. If relays are reachable, send over the internet.
//! 2. If not, and the contact's radio has been heard on a mesh and preset the user's radio
//!    shares, send over that radio.
//! 3. Otherwise the message waits, and goes out when a relay is back.
//!
//! The switch that forces radio (#40) comes before the rules, for testing.
//!
//! A message never goes out on both paths at once by choice. Every message is MDK's from the
//! start, though, and MDK publishes it once a relay can be reached: for a message that went by
//! radio, that copy is the permanent record (#47). If the relays come back while the radio is
//! still sending, both copies arrive, the other phone keeps one, and the message stays marked as
//! radio, the way it went first.

use crate::interface::RadioIssue;

/// How a message goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Internet,
    Radio,
    /// No path now: MDK keeps the message until a relay is back.
    Wait,
}

/// The route for a message: with radio `forced` or not, with the chat's relays reachable or
/// not, and with the contact's radio reachable or why not.
pub fn route(forced: bool, relays_up: bool, radio: Result<(), RadioIssue>) -> Route {
    if forced {
        Route::Radio
    } else if relays_up {
        Route::Internet
    } else if radio.is_ok() {
        Route::Radio
    } else {
        Route::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RadioIssue::*;
    use Route::{Internet, Radio, Wait};

    /// Every combination: forced, relays up, the contact's radio, and the route.
    const TABLE: [(bool, bool, Result<(), RadioIssue>, Route); 24] = [
        // Rule 1: the internet while a relay is reachable, also when radio is.
        (false, true, Ok(()), Internet),
        (false, true, Err(NoRadio), Internet),
        (false, true, Err(NoContactRadio), Internet),
        (false, true, Err(OtherMesh), Internet),
        (false, true, Err(OtherPreset), Internet),
        (false, true, Err(NotHeard), Internet),
        // Rule 2: radio when no relay is, and the contact's radio was heard on the same preset.
        (false, false, Ok(()), Radio),
        // Rule 3: otherwise the message waits.
        (false, false, Err(NoRadio), Wait),
        (false, false, Err(NoContactRadio), Wait),
        (false, false, Err(OtherMesh), Wait),
        (false, false, Err(OtherPreset), Wait),
        (false, false, Err(NotHeard), Wait),
        // The switch sends by radio, whatever holds; the radio then says why it can't.
        (true, true, Ok(()), Radio),
        (true, true, Err(NoRadio), Radio),
        (true, true, Err(NoContactRadio), Radio),
        (true, true, Err(OtherMesh), Radio),
        (true, true, Err(OtherPreset), Radio),
        (true, true, Err(NotHeard), Radio),
        (true, false, Ok(()), Radio),
        (true, false, Err(NoRadio), Radio),
        (true, false, Err(NoContactRadio), Radio),
        (true, false, Err(OtherMesh), Radio),
        (true, false, Err(OtherPreset), Radio),
        (true, false, Err(NotHeard), Radio),
    ];

    #[test]
    fn every_state_has_its_route() {
        for (forced, relays_up, radio, expected) in TABLE {
            assert_eq!(
                route(forced, relays_up, radio),
                expected,
                "forced {forced}, relays up {relays_up}, radio {radio:?}"
            );
        }
    }
}
