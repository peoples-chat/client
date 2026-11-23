//! Which radio packets to accept (#42): the receiving side's checks, before anything reaches a
//! chat.
//!
//! A packet is accepted only if it opens with a kept key of one of the user's direct chats
//! (member-level authentication), its counter is new to the replay window, and the message it
//! carries isn't known already. Everything else is refused, and a refusal goes nowhere: the
//! inbox can't transmit, and its caller shows nothing and sends nothing for it.
//!
//! The replay windows live in memory. After a restart, a replayed packet is still caught by its
//! message id, since received radio messages are stored with it (#40). The rules are those of
//! `docs/radio-format-v0.md`.

use std::collections::HashMap;

use super::format::{self, Header, PacketError};
use super::keys::{RadioKey, RadioKeys};

/// How many epochs' keys a chat keeps (spike #37).
pub const KEPT_EPOCHS: usize = 4;
/// How far below the highest counter a packet may still arrive (the format's replay window).
pub const REPLAY_WINDOW: u64 = 64;

/// A text that passed every check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedText {
    pub chat_id: String,
    /// MDK's id for the message, which its relay copy will have too.
    pub message_id: String,
    pub text: String,
    /// Seconds since the Unix epoch.
    pub sent_at: u32,
    pub epoch: u64,
    pub counter: u64,
}

/// Why a packet was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("malformed: {0}")]
    Malformed(PacketError),
    #[error("no direct chat to receive in")]
    UnknownChat,
    #[error("no kept key for the packet's epoch")]
    UnknownEpoch,
    #[error("the packet doesn't open with any kept key")]
    NotAuthentic,
    #[error("the packet was received before, or is too old")]
    Replayed,
    #[error("the message is known already")]
    Duplicate,
}

/// The counters seen in one direction of one chat in one epoch: the highest, and which of the
/// [`REPLAY_WINDOW`] below it.
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

struct Chat {
    /// The contact's account key, in hex: the sender of what arrives.
    contact: String,
    /// Newest first.
    keys: Vec<RadioKeys>,
    windows: HashMap<u64, ReplayWindow>,
}

/// The user's direct chats as radio receives them: their kept keys and replay windows.
#[derive(Default)]
pub struct RadioInbox {
    chats: HashMap<String, Chat>,
}

impl RadioInbox {
    /// Keeps `keys` for the chat `chat_id` with `contact`. A chat keeps its [`KEPT_EPOCHS`]
    /// newest epochs; keys for an epoch it has already are replaced.
    pub fn keep_keys(&mut self, chat_id: &str, contact: &str, keys: RadioKeys) {
        let chat = self
            .chats
            .entry(chat_id.to_owned())
            .or_insert_with(|| Chat {
                contact: contact.to_owned(),
                keys: Vec::new(),
                windows: HashMap::new(),
            });
        chat.contact = contact.to_owned();
        chat.keys.retain(|kept| kept.epoch != keys.epoch);
        chat.keys.push(keys);
        chat.keys.sort_by_key(|keys| std::cmp::Reverse(keys.epoch));
        chat.keys.truncate(KEPT_EPOCHS);
        let kept: Vec<u64> = chat.keys.iter().map(|keys| keys.epoch).collect();
        chat.windows.retain(|epoch, _| kept.contains(epoch));
    }

    /// The epochs the chat keeps keys for, newest first.
    pub fn kept_epochs(&self, chat_id: &str) -> Vec<u64> {
        self.chats.get(chat_id).map_or_else(Vec::new, |chat| {
            chat.keys.iter().map(|keys| keys.epoch).collect()
        })
    }

    /// What a packet in the chat goes out with: its newest kept epoch, and the key this phone
    /// sends with in it (#40). The format's sender uses the newest epoch it holds keys for.
    pub fn sending_key(&self, chat_id: &str) -> Option<(u64, RadioKey)> {
        let keys = self.chats.get(chat_id)?.keys.first()?;
        Some((keys.epoch, keys.send.clone()))
    }

    /// Forgets every chat, as when the account is deleted.
    pub fn clear(&mut self) {
        self.chats.clear();
    }

    /// Checks a packet that arrived over radio, and records it when it passes.
    ///
    /// `from_chat` is the chat of the radio that sent it, when the radio is known (#34);
    /// otherwise every chat's keys are tried. `known` tells whether a message id is in the chat
    /// already, for example because its relay copy came first.
    pub fn receive(
        &mut self,
        packet: &[u8],
        from_chat: Option<&str>,
        known: impl Fn(&str, &str) -> bool,
    ) -> Result<ReceivedText, Refusal> {
        let header = Header::parse(packet).map_err(Refusal::Malformed)?;
        let candidates: Vec<String> = match from_chat {
            Some(chat_id) if self.chats.contains_key(chat_id) => vec![chat_id.to_owned()],
            Some(_) => Vec::new(),
            None => self.chats.keys().cloned().collect(),
        };
        if candidates.is_empty() {
            return Err(Refusal::UnknownChat);
        }

        let mut refusal = Refusal::UnknownEpoch;
        for chat_id in candidates {
            let chat = self.chats.get_mut(&chat_id).expect("a candidate is a chat");
            for keys in chat
                .keys
                .iter()
                .filter(|keys| keys.epoch as u8 == header.epoch_hint)
            {
                let window = chat.windows.entry(keys.epoch).or_default();
                let counter = format::full_counter(window.highest, header.counter_low);
                let opened = match format::open_text(&keys.receive, packet, counter) {
                    Ok(opened) => opened,
                    Err(_) => {
                        refusal = Refusal::NotAuthentic;
                        continue;
                    }
                };
                // Authentic. Only now does the packet count against the window.
                if !window.is_new(counter) {
                    return Err(Refusal::Replayed);
                }
                window.record(counter);
                let message_id = format::message_id(&chat.contact, opened.sent_at, &opened.text);
                if known(&chat_id, &message_id) {
                    return Err(Refusal::Duplicate);
                }
                return Ok(ReceivedText {
                    chat_id,
                    message_id,
                    text: opened.text,
                    sent_at: opened.sent_at,
                    epoch: keys.epoch,
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

    use super::*;
    use crate::radio::format::seal_text;

    const ADA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BEA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const CAI: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    /// The chat secret of a chat in an epoch, made up for the tests.
    fn secret(chat: u8, epoch: u64) -> Vec<u8> {
        (0..32u8)
            .map(|byte| byte ^ chat ^ epoch as u8 ^ (epoch >> 8) as u8)
            .collect()
    }

    fn keys(own: &str, contact: &str, chat: u8, epoch: u64) -> RadioKeys {
        RadioKeys::derive(&secret(chat, epoch), epoch, own, contact).unwrap()
    }

    /// Bea's inbox, with her chat with Ada (chat 1) in `epochs`.
    fn bea_inbox(epochs: &[u64]) -> RadioInbox {
        let mut inbox = RadioInbox::default();
        for epoch in epochs {
            inbox.keep_keys("chat-ada", ADA, keys(BEA, ADA, 1, *epoch));
        }
        inbox
    }

    /// A packet from Ada to Bea in chat 1.
    fn from_ada(epoch: u64, counter: u64, text: &str) -> Vec<u8> {
        let ada = keys(ADA, BEA, 1, epoch);
        seal_text(&ada.send, epoch, counter, 1795300000 + counter as u32, text)
    }

    fn nothing_known(_: &str, _: &str) -> bool {
        false
    }

    #[test]
    fn accepts_a_packet_once() {
        let mut inbox = bea_inbox(&[7]);
        let packet = from_ada(7, 0, "Hello Bea");

        let received = inbox
            .receive(&packet, Some("chat-ada"), nothing_known)
            .unwrap();
        assert_eq!(received.chat_id, "chat-ada");
        assert_eq!(received.text, "Hello Bea");
        assert_eq!(received.epoch, 7);
        assert_eq!(received.counter, 0);
        assert_eq!(
            received.message_id,
            format::message_id(ADA, 1795300000, "Hello Bea")
        );

        // The same packet, captured and sent again.
        assert_eq!(
            inbox.receive(&packet, Some("chat-ada"), nothing_known),
            Err(Refusal::Replayed)
        );
    }

    #[test]
    fn a_forgery_changes_nothing() {
        let mut inbox = bea_inbox(&[7]);
        let packet = from_ada(7, 0, "Hello Bea");
        let mut forged = packet.clone();
        let last = forged.len() - 1;
        forged[last] ^= 1;
        assert_eq!(
            inbox.receive(&forged, Some("chat-ada"), nothing_known),
            Err(Refusal::NotAuthentic)
        );
        // It didn't use up the counter.
        assert!(
            inbox
                .receive(&packet, Some("chat-ada"), nothing_known)
                .is_ok()
        );

        // A packet Bea sent herself, reflected back to her.
        let bea = keys(BEA, ADA, 1, 7);
        let own = seal_text(&bea.send, 7, 1, 1795300001, "Hi Ada");
        assert_eq!(
            inbox.receive(&own, Some("chat-ada"), nothing_known),
            Err(Refusal::NotAuthentic)
        );
    }

    #[test]
    fn the_replay_window_takes_late_packets_once() {
        let mut inbox = bea_inbox(&[7]);
        for counter in [5, 3, 4, 70] {
            assert!(
                inbox
                    .receive(
                        &from_ada(7, counter, "late"),
                        Some("chat-ada"),
                        nothing_known
                    )
                    .is_ok(),
                "{counter}"
            );
        }
        // The window now holds 7 to 70. What was seen in it, and anything older, is refused.
        for (counter, refusal) in [
            (3, Refusal::Replayed),
            (5, Refusal::Replayed),
            (70, Refusal::Replayed),
            (6, Refusal::Replayed),
        ] {
            let result = inbox.receive(
                &from_ada(7, counter, "late"),
                Some("chat-ada"),
                nothing_known,
            );
            assert_eq!(result, Err(refusal), "{counter}");
        }
        assert!(
            inbox
                .receive(&from_ada(7, 7, "late"), Some("chat-ada"), nothing_known)
                .is_ok()
        );
    }

    #[test]
    fn counters_carry_on_past_their_low_bits() {
        let mut inbox = bea_inbox(&[7]);
        for counter in [65534, 65535, 65536, 65537, 70000] {
            let received = inbox
                .receive(&from_ada(7, counter, "on"), Some("chat-ada"), nothing_known)
                .unwrap();
            assert_eq!(received.counter, counter);
        }
    }

    #[test]
    fn refuses_unknown_epochs_and_keeps_four() {
        let mut inbox = bea_inbox(&[7]);
        assert_eq!(
            inbox.receive(&from_ada(8, 0, "new"), Some("chat-ada"), nothing_known),
            Err(Refusal::UnknownEpoch)
        );
        // Epoch 263 has the same hint as 7, but another key.
        assert_eq!(
            inbox.receive(&from_ada(263, 0, "far"), Some("chat-ada"), nothing_known),
            Err(Refusal::NotAuthentic)
        );

        for epoch in 8..=11 {
            inbox.keep_keys("chat-ada", ADA, keys(BEA, ADA, 1, epoch));
        }
        assert_eq!(inbox.kept_epochs("chat-ada"), vec![11, 10, 9, 8]);
        assert!(
            inbox
                .receive(&from_ada(8, 0, "new"), Some("chat-ada"), nothing_known)
                .is_ok()
        );
        assert_eq!(
            inbox.receive(&from_ada(7, 1, "old"), Some("chat-ada"), nothing_known),
            Err(Refusal::UnknownEpoch)
        );
    }

    #[test]
    fn finds_the_chat_when_the_radio_is_unknown() {
        let mut inbox = bea_inbox(&[7]);
        inbox.keep_keys("chat-cai", CAI, keys(BEA, CAI, 2, 3));
        let cai = keys(CAI, BEA, 2, 3);
        let packet = seal_text(&cai.send, 3, 0, 1795300000, "From Cai");

        let received = inbox.receive(&packet, None, nothing_known).unwrap();
        assert_eq!(received.chat_id, "chat-cai");
        assert_eq!(
            received.message_id,
            format::message_id(CAI, 1795300000, "From Cai")
        );
        // Named as coming from Ada's radio, it doesn't open.
        let again = seal_text(&cai.send, 3, 1, 1795300001, "From Cai");
        assert_eq!(
            inbox.receive(&again, Some("chat-ada"), nothing_known),
            Err(Refusal::UnknownEpoch)
        );
        assert_eq!(
            inbox.receive(&again, Some("chat-unknown"), nothing_known),
            Err(Refusal::UnknownChat)
        );
        assert_eq!(
            RadioInbox::default().receive(&again, None, nothing_known),
            Err(Refusal::UnknownChat)
        );
    }

    #[test]
    fn refuses_a_message_the_chat_has_already() {
        let mut inbox = bea_inbox(&[7]);
        let packet = from_ada(7, 0, "Hello Bea");
        let id = format::message_id(ADA, 1795300000, "Hello Bea");
        let history: HashSet<(String, String)> = [("chat-ada".to_owned(), id)].into();
        let known = |chat: &str, id: &str| history.contains(&(chat.to_owned(), id.to_owned()));
        assert_eq!(
            inbox.receive(&packet, Some("chat-ada"), known),
            Err(Refusal::Duplicate)
        );
        // Re-sent after a restart, when the window is new but the history isn't.
        let mut restarted = bea_inbox(&[7]);
        assert_eq!(
            restarted.receive(&packet, Some("chat-ada"), known),
            Err(Refusal::Duplicate)
        );
    }

    #[test]
    fn sends_with_the_newest_kept_epoch() {
        let mut inbox = bea_inbox(&[7, 9, 8]);
        let (epoch, key) = inbox.sending_key("chat-ada").unwrap();
        assert_eq!(epoch, 9);
        assert_eq!(*key, *keys(BEA, ADA, 1, 9).send);
        assert!(inbox.sending_key("chat-cai").is_none());
        inbox.clear();
        assert!(inbox.sending_key("chat-ada").is_none());
    }

    #[test]
    fn refuses_malformed_packets() {
        let mut inbox = bea_inbox(&[7]);
        assert_eq!(
            inbox.receive(&[0; 5], Some("chat-ada"), nothing_known),
            Err(Refusal::Malformed(PacketError::TooShort(5)))
        );
    }
}
