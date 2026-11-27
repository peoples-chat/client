//! Which radio packets to accept (#42): the receiving side's checks, before anything reaches a
//! chat.
//!
//! A packet is accepted only if it opens with a kept key of the contact in one of the user's
//! direct chats (member-level authentication), its counter is new to that key's replay window,
//! and the message it carries isn't known already. Everything else is refused, and a refusal goes
//! nowhere: the inbox can't transmit, and its caller shows nothing and sends nothing for it.
//!
//! The keys are the ones the contact sent in the chat (#128); the radio store keeps them, and
//! hands the inbox those for the mesh of the radio in use. The replay windows live in memory.
//! After a restart, a replayed packet is still caught by its message id, since received radio
//! messages are stored with it (#40). The rules are those of `docs/radio-format-v0.md`.

use std::collections::HashMap;

use super::format::{self, Header, PacketError};
use super::keys::{RADIO_KEY_BYTES, RadioKey};

/// How far below the highest counter a packet may still arrive (the format's replay window).
pub const REPLAY_WINDOW: u64 = 64;

/// One of a contact's keys, as the inbox may use it. `Debug` leaves the key out.
#[derive(Clone)]
pub struct InboxKey {
    /// The key's number; a packet carries its lowest 8 bits.
    pub number: u64,
    pub key: RadioKey,
    /// For a key that a newer one replaced: when the contact made the newer one, in seconds since
    /// the Unix epoch by their clock. A packet sent after that is refused.
    pub replaced_at: Option<u32>,
}

impl std::fmt::Debug for InboxKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxKey")
            .field("number", &self.number)
            .field("replaced_at", &self.replaced_at)
            .finish_non_exhaustive()
    }
}

/// A direct chat as radio receives it.
#[derive(Clone, Debug)]
pub struct ChatKeys {
    pub chat_id: String,
    /// The contact's account key, in hex: the sender of what arrives.
    pub contact: String,
    /// The contact's keys for the mesh in use.
    pub keys: Vec<InboxKey>,
}

/// A text that passed every check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedText {
    pub chat_id: String,
    /// MDK's id for the message, which its relay copy will have too.
    pub message_id: String,
    pub text: String,
    /// Seconds since the Unix epoch.
    pub sent_at: u32,
    pub key_number: u64,
    pub counter: u64,
}

/// Why a packet was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("malformed: {0}")]
    Malformed(PacketError),
    #[error("no direct chat to receive in")]
    UnknownChat,
    #[error("no kept key with the packet's number")]
    UnknownKey,
    #[error("the packet doesn't open with any kept key")]
    NotAuthentic,
    #[error("the packet was sent with a key that had been replaced")]
    ReplacedKey,
    #[error("the packet was received before, or is too old")]
    Replayed,
    #[error("the message is known already")]
    Duplicate,
}

/// The counters seen under one key: the highest, and which of the [`REPLAY_WINDOW`] below it.
#[derive(Clone, Copy, Debug, Default)]
struct ReplayWindow {
    highest: Option<u64>,
    /// Bit `n` is set when counter `highest - n` was seen.
    seen: u64,
}

impl ReplayWindow {
    fn is_new(&self, counter: u64) -> bool {
        match self.highest {
            None => true,
            Some(highest) if counter > highest => true,
            Some(highest) => {
                let behind = highest - counter;
                behind < REPLAY_WINDOW && self.seen & (1 << behind) == 0
            }
        }
    }

    fn record(&mut self, counter: u64) {
        match self.highest {
            Some(highest) if counter <= highest => self.seen |= 1 << (highest - counter),
            Some(highest) => {
                let ahead = counter - highest;
                self.seen = if ahead >= REPLAY_WINDOW {
                    0
                } else {
                    self.seen << ahead
                } | 1;
                self.highest = Some(counter);
            }
            None => {
                self.seen = 1;
                self.highest = Some(counter);
            }
        }
    }
}

/// The replay windows of the keys that radio receives with.
#[derive(Default)]
pub struct RadioInbox {
    /// By chat and key.
    windows: HashMap<(String, [u8; RADIO_KEY_BYTES]), ReplayWindow>,
}

impl RadioInbox {
    /// Forgets every window, as when the account is deleted.
    pub fn clear(&mut self) {
        self.windows.clear();
    }

    /// Checks a packet that arrived over radio, and records it when it passes.
    ///
    /// `chats` are the user's direct chats with the keys their contacts sent. `from_chat` is the
    /// chat of the radio that sent the packet, when the radio is known (#34); otherwise every
    /// chat's keys are tried. `known` tells whether a message id is in the chat already, for
    /// example because its relay copy came first.
    pub fn receive(
        &mut self,
        packet: &[u8],
        chats: &[ChatKeys],
        from_chat: Option<&str>,
        known: impl Fn(&str, &str) -> bool,
    ) -> Result<ReceivedText, Refusal> {
        let header = Header::parse(packet).map_err(Refusal::Malformed)?;
        let mut candidates = chats
            .iter()
            .filter(|chat| from_chat.is_none_or(|from| from == chat.chat_id))
            .peekable();
        if candidates.peek().is_none() {
            return Err(Refusal::UnknownChat);
        }

        let mut refusal = Refusal::UnknownKey;
        for chat in candidates {
            for kept in chat
                .keys
                .iter()
                .filter(|kept| kept.number as u8 == header.key_hint)
            {
                let of_key = (chat.chat_id.clone(), *kept.key);
                let highest = self.windows.get(&of_key).and_then(|window| window.highest);
                let counter = format::full_counter(highest, header.counter_low);
                let Ok(opened) = format::open_text(&kept.key, packet, counter) else {
                    refusal = Refusal::NotAuthentic;
                    continue;
                };
                if kept.replaced_at.is_some_and(|at| opened.sent_at > at) {
                    return Err(Refusal::ReplacedKey);
                }
                // Authentic. Only now does the packet count against the window.
                let window = self.windows.entry(of_key).or_default();
                if !window.is_new(counter) {
                    return Err(Refusal::Replayed);
                }
                window.record(counter);
                let message_id = format::message_id(&chat.contact, opened.sent_at, &opened.text);
                if known(&chat.chat_id, &message_id) {
                    return Err(Refusal::Duplicate);
                }
                return Ok(ReceivedText {
                    chat_id: chat.chat_id.clone(),
                    message_id,
                    text: opened.text,
                    sent_at: opened.sent_at,
                    key_number: kept.number,
                    counter,
                });
            }
        }
        Err(refusal)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use zeroize::Zeroizing;

    use super::*;
    use crate::radio::format::seal_text;

    const ADA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CAI: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    /// Key number `number` of the member `who`, made up for the tests.
    fn key(who: u8, number: u64) -> InboxKey {
        InboxKey {
            number,
            key: Zeroizing::new(std::array::from_fn(|byte| {
                byte as u8 ^ who ^ number as u8 ^ (number >> 8) as u8
            })),
            replaced_at: None,
        }
    }

    /// Bea's chat with Ada (member 1), with Ada's keys `numbers`.
    fn with_ada(numbers: &[u64]) -> Vec<ChatKeys> {
        vec![ChatKeys {
            chat_id: "chat-ada".to_owned(),
            contact: ADA.to_owned(),
            keys: numbers.iter().map(|number| key(1, *number)).collect(),
        }]
    }

    /// A packet from Ada to Bea.
    fn from_ada(number: u64, counter: u64, text: &str) -> Vec<u8> {
        let sent_at = 1795300000 + counter as u32;
        seal_text(&key(1, number).key, number, counter, sent_at, text)
    }

    fn nothing_known(_: &str, _: &str) -> bool {
        false
    }

    fn receive(
        inbox: &mut RadioInbox,
        chats: &[ChatKeys],
        packet: &[u8],
    ) -> Result<ReceivedText, Refusal> {
        inbox.receive(packet, chats, Some("chat-ada"), nothing_known)
    }

    #[test]
    fn accepts_a_packet_once() {
        let mut inbox = RadioInbox::default();
        let chats = with_ada(&[7]);
        let packet = from_ada(7, 0, "Hello Bea");

        let received = receive(&mut inbox, &chats, &packet).unwrap();
        assert_eq!(received.chat_id, "chat-ada");
        assert_eq!(received.text, "Hello Bea");
        assert_eq!(received.key_number, 7);
        assert_eq!(received.counter, 0);
        assert_eq!(
            received.message_id,
            format::message_id(ADA, 1795300000, "Hello Bea")
        );

        // The same packet, captured and sent again.
        assert_eq!(receive(&mut inbox, &chats, &packet), Err(Refusal::Replayed));
    }

    #[test]
    fn a_forgery_changes_nothing() {
        let mut inbox = RadioInbox::default();
        let chats = with_ada(&[7]);
        let packet = from_ada(7, 0, "Hello Bea");
        let mut forged = packet.clone();
        let last = forged.len() - 1;
        forged[last] ^= 1;
        assert_eq!(
            receive(&mut inbox, &chats, &forged),
            Err(Refusal::NotAuthentic)
        );
        // It didn't use up the counter.
        assert!(receive(&mut inbox, &chats, &packet).is_ok());

        // A packet Bea sealed with her own key, reflected back to her.
        let own = seal_text(&key(2, 7).key, 7, 1, 1795300001, "Hi Ada");
        assert_eq!(
            receive(&mut inbox, &chats, &own),
            Err(Refusal::NotAuthentic)
        );
    }

    #[test]
    fn the_replay_window_takes_late_packets_once() {
        let mut inbox = RadioInbox::default();
        let chats = with_ada(&[7]);
        for counter in [5, 3, 4, 70] {
            let packet = from_ada(7, counter, "late");
            assert!(receive(&mut inbox, &chats, &packet).is_ok(), "{counter}");
        }
        // The window now holds 7 to 70. What was seen in it, and anything older, is refused.
        for counter in [3, 5, 70, 6] {
            let packet = from_ada(7, counter, "late");
            assert_eq!(
                receive(&mut inbox, &chats, &packet),
                Err(Refusal::Replayed),
                "{counter}"
            );
        }
        assert!(receive(&mut inbox, &chats, &from_ada(7, 7, "late")).is_ok());
    }

    #[test]
    fn counters_carry_on_past_their_low_bits() {
        let mut inbox = RadioInbox::default();
        let chats = with_ada(&[7]);
        for counter in [65534, 65535, 65536, 65537, 70000] {
            let received = receive(&mut inbox, &chats, &from_ada(7, counter, "on")).unwrap();
            assert_eq!(received.counter, counter);
        }
    }

    #[test]
    fn picks_the_key_by_its_number_and_drops_unknown_ones() {
        let mut inbox = RadioInbox::default();
        let chats = with_ada(&[7, 8]);
        assert_eq!(
            receive(&mut inbox, &chats, &from_ada(9, 0, "new")),
            Err(Refusal::UnknownKey)
        );
        // Number 263 has the same low bits as 7, but another key.
        assert_eq!(
            receive(&mut inbox, &chats, &from_ada(263, 0, "far")),
            Err(Refusal::NotAuthentic)
        );
        // Each key has a window of its own.
        for number in [7, 8] {
            let received = receive(&mut inbox, &chats, &from_ada(number, 0, "mine")).unwrap();
            assert_eq!(received.key_number, number);
        }
    }

    #[test]
    fn a_replaced_key_opens_only_what_was_sent_before_the_newer_one() {
        let mut inbox = RadioInbox::default();
        let mut chats = with_ada(&[7, 8]);
        chats[0].keys[0].replaced_at = Some(1795300005);
        // In flight when the key changed.
        assert!(receive(&mut inbox, &chats, &from_ada(7, 5, "before")).is_ok());
        // Sealed with the old key afterwards.
        assert_eq!(
            receive(&mut inbox, &chats, &from_ada(7, 6, "after")),
            Err(Refusal::ReplacedKey)
        );
        assert!(receive(&mut inbox, &chats, &from_ada(8, 0, "after")).is_ok());
    }

    #[test]
    fn finds_the_chat_when_the_radio_is_unknown() {
        let mut inbox = RadioInbox::default();
        let mut chats = with_ada(&[7]);
        chats.push(ChatKeys {
            chat_id: "chat-cai".to_owned(),
            contact: CAI.to_owned(),
            keys: vec![key(3, 3)],
        });
        let packet = seal_text(&key(3, 3).key, 3, 0, 1795300000, "From Cai");

        let received = inbox.receive(&packet, &chats, None, nothing_known).unwrap();
        assert_eq!(received.chat_id, "chat-cai");
        assert_eq!(
            received.message_id,
            format::message_id(CAI, 1795300000, "From Cai")
        );
        // Named as coming from Ada's radio, it doesn't open.
        let again = seal_text(&key(3, 3).key, 3, 1, 1795300001, "From Cai");
        assert_eq!(
            inbox.receive(&again, &chats, Some("chat-ada"), nothing_known),
            Err(Refusal::UnknownKey)
        );
        assert_eq!(
            inbox.receive(&again, &chats, Some("chat-unknown"), nothing_known),
            Err(Refusal::UnknownChat)
        );
        assert_eq!(
            inbox.receive(&again, &[], None, nothing_known),
            Err(Refusal::UnknownChat)
        );
    }

    #[test]
    fn refuses_a_message_the_chat_has_already() {
        let mut inbox = RadioInbox::default();
        let chats = with_ada(&[7]);
        let packet = from_ada(7, 0, "Hello Bea");
        let id = format::message_id(ADA, 1795300000, "Hello Bea");
        let history: HashSet<(String, String)> = [("chat-ada".to_owned(), id)].into();
        let known = |chat: &str, id: &str| history.contains(&(chat.to_owned(), id.to_owned()));
        assert_eq!(
            inbox.receive(&packet, &chats, Some("chat-ada"), known),
            Err(Refusal::Duplicate)
        );
        // Re-sent after a restart, when the window is new but the history isn't.
        inbox.clear();
        assert_eq!(
            inbox.receive(&packet, &chats, Some("chat-ada"), known),
            Err(Refusal::Duplicate)
        );
    }

    #[test]
    fn refuses_malformed_packets() {
        let mut inbox = RadioInbox::default();
        assert_eq!(
            receive(&mut inbox, &with_ada(&[7]), &[0; 5]),
            Err(Refusal::Malformed(PacketError::TooShort(5)))
        );
    }
}
