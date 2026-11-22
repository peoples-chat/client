//! Which chats can reach their relays now (#43).
//!
//! MDK reports when the account's connection to a relay opens or closes. A relay counts as gone
//! once its connection has stayed closed for [`DOWN_AFTER`], and as back as soon as it connects
//! again. MDK tries a closed connection every 5 seconds, so a drop shorter than that doesn't make
//! a chat flicker. A chat reaches the internet while any of its relays does.
//!
//! The phone's own online flag plays no part: a phone can be online while no relay answers, for
//! example behind a captive portal, and the relays are what carry the messages.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use crate::interface::Reach;
use crate::messaging::RelayLink;

/// How long a relay's connection must stay closed before the relay counts as gone.
pub const DOWN_AFTER: Duration = Duration::from_secs(10);

/// The account's relays, and the relays each chat goes through.
#[derive(Debug, Default)]
pub struct Reachability {
    relays: HashMap<String, Link>,
    chats: HashMap<String, Vec<String>>,
}

#[derive(Clone, Copy, Debug)]
struct Link {
    connected: bool,
    /// When the connection last opened or closed, or when following began.
    since: Instant,
}

impl Link {
    fn up(&self, now: Instant) -> bool {
        self.connected || now < self.since + DOWN_AFTER
    }
}

impl Reachability {
    /// Starts from the relays' connections at `now`. A relay that isn't connected yet gets
    /// [`DOWN_AFTER`] to connect, as after a drop.
    pub fn new(links: &[RelayLink], now: Instant) -> Self {
        let mut reachability = Self::default();
        reachability.update(links, now);
        reachability
    }

    /// Takes the relays' connections as they are at `now`. Returns whether a relay connected
    /// again after it had counted as gone, so that messages waiting to go out can go now.
    pub fn update(&mut self, links: &[RelayLink], now: Instant) -> bool {
        let mut back = false;
        let mut relays = HashMap::with_capacity(links.len());
        for link in links {
            let next = match self.relays.get(&link.url) {
                Some(known) if known.connected == link.connected => *known,
                known => {
                    back |= link.connected && known.is_some_and(|known| !known.up(now));
                    Link {
                        connected: link.connected,
                        since: now,
                    }
                }
            };
            relays.insert(link.url.clone(), next);
        }
        self.relays = relays;
        back
    }

    /// Records the relays a chat goes through. Nothing changes when `relays` is empty.
    pub fn set_chat_relays(&mut self, chat_id: &str, relays: &[String]) {
        if !relays.is_empty() {
            self.chats.insert(chat_id.to_owned(), relays.to_vec());
        }
    }

    /// How the chat's messages can travel at `now`: over the internet or not at all, until radio
    /// comes in (#44).
    ///
    /// A chat follows its own relays where they are among the account's. Otherwise, for example
    /// when someone else's app chose them, it follows all of the account's relays: MDK reports
    /// only those.
    pub fn reach(&self, chat_id: &str, now: Instant) -> Reach {
        let own: Vec<&Link> = self
            .chats
            .get(chat_id)
            .into_iter()
            .flatten()
            .filter_map(|relay| self.relays.get(relay))
            .collect();
        let links = if own.is_empty() {
            self.relays.values().collect()
        } else {
            own
        };
        // Without any relay to go by there is nothing to report.
        if links.is_empty() || links.iter().any(|link| link.up(now)) {
            Reach::Internet
        } else {
            Reach::Unreachable
        }
    }

    /// The soonest moment after `now` at which a closed connection makes its relay count as gone.
    pub fn next_change(&self, now: Instant) -> Option<Instant> {
        self.relays
            .values()
            .filter(|link| !link.connected)
            .map(|link| link.since + DOWN_AFTER)
            .filter(|at| *at > now)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    fn links(relays: &[(&str, bool)]) -> Vec<RelayLink> {
        relays
            .iter()
            .map(|(url, connected)| RelayLink {
                url: (*url).to_owned(),
                connected: *connected,
            })
            .collect()
    }

    #[test]
    fn a_chat_is_cut_off_once_its_relays_have_been_gone_for_a_while() {
        let start = Instant::now();
        let mut relays = Reachability::new(&links(&[("wss://a", true), ("wss://b", true)]), start);
        assert_eq!(relays.reach("chat", start), Reach::Internet);
        assert_eq!(relays.next_change(start), None);

        // One relay drops: the other still carries the chat.
        relays.update(&links(&[("wss://a", false), ("wss://b", true)]), start);
        assert_eq!(relays.reach("chat", start + 60 * SECOND), Reach::Internet);

        // The other drops 2 seconds later. The chat is cut off once both have been gone for
        // DOWN_AFTER.
        let dropped = start + 2 * SECOND;
        relays.update(&links(&[("wss://a", false), ("wss://b", false)]), dropped);
        assert_eq!(relays.next_change(dropped), Some(start + DOWN_AFTER));
        assert_eq!(
            relays.reach("chat", dropped + DOWN_AFTER - SECOND),
            Reach::Internet
        );
        assert_eq!(
            relays.reach("chat", dropped + DOWN_AFTER),
            Reach::Unreachable
        );
        assert_eq!(relays.next_change(dropped + DOWN_AFTER), None);

        // Either one connecting brings the chat back at once, and says the relays are back.
        let back = dropped + 30 * SECOND;
        assert!(relays.update(&links(&[("wss://a", false), ("wss://b", true)]), back));
        assert_eq!(relays.reach("chat", back), Reach::Internet);
    }

    #[test]
    fn a_short_drop_changes_nothing() {
        let start = Instant::now();
        let mut relays = Reachability::new(&links(&[("wss://a", true)]), start);
        relays.update(&links(&[("wss://a", false)]), start + SECOND);
        // MDK tries again every 5 seconds, and the attempt succeeds.
        let again = start + 6 * SECOND;
        assert!(!relays.update(&links(&[("wss://a", true)]), again));
        for seconds in 0..30 {
            assert_eq!(
                relays.reach("chat", start + seconds * SECOND),
                Reach::Internet
            );
        }
        assert_eq!(relays.next_change(again), None);
    }

    #[test]
    fn retries_that_fail_do_not_restart_the_wait() {
        let start = Instant::now();
        let mut relays = Reachability::new(&links(&[("wss://a", true)]), start);
        relays.update(&links(&[("wss://a", false)]), start);
        // Connecting and failing again reports "not connected" each time.
        relays.update(&links(&[("wss://a", false)]), start + 5 * SECOND);
        assert_eq!(relays.reach("chat", start + DOWN_AFTER), Reach::Unreachable);
    }

    #[test]
    fn relays_that_never_connect_count_as_gone_after_the_wait() {
        let start = Instant::now();
        let relays = Reachability::new(&links(&[("wss://a", false)]), start);
        assert_eq!(relays.reach("chat", start), Reach::Internet);
        assert_eq!(relays.next_change(start), Some(start + DOWN_AFTER));
        assert_eq!(relays.reach("chat", start + DOWN_AFTER), Reach::Unreachable);
    }

    #[test]
    fn a_chat_follows_its_own_relays_where_the_account_has_them() {
        let start = Instant::now();
        let mut relays = Reachability::new(&links(&[("wss://a", false), ("wss://b", true)]), start);
        relays.set_chat_relays("on-a", &["wss://a".to_owned()]);
        relays.set_chat_relays("on-a-and-b", &["wss://a".to_owned(), "wss://b".to_owned()]);
        relays.set_chat_relays("elsewhere", &["wss://c".to_owned()]);
        // A chat the core made itself names no relays, and keeps what it had.
        relays.set_chat_relays("on-a", &[]);

        let later = start + DOWN_AFTER;
        assert_eq!(relays.reach("on-a", later), Reach::Unreachable);
        assert_eq!(relays.reach("on-a-and-b", later), Reach::Internet);
        // Unknown relays and unknown chats go by all of the account's relays.
        assert_eq!(relays.reach("elsewhere", later), Reach::Internet);
        assert_eq!(relays.reach("unknown", later), Reach::Internet);
    }

    #[test]
    fn without_relays_there_is_nothing_to_report() {
        let relays = Reachability::default();
        assert_eq!(relays.reach("chat", Instant::now()), Reach::Internet);
        assert_eq!(relays.next_change(Instant::now()), None);
    }

    #[test]
    fn a_relay_that_comes_back_before_it_counted_as_gone_is_not_news() {
        let start = Instant::now();
        let mut relays = Reachability::new(&links(&[("wss://a", false)]), start);
        assert!(!relays.update(&links(&[("wss://a", true)]), start + 2 * SECOND));
        // A relay the account just added is new, not back.
        assert!(!relays.update(
            &links(&[("wss://a", true), ("wss://b", true)]),
            start + 60 * SECOND
        ));
    }
}
