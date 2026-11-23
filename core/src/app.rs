//! The core as an interface sees it (#16): a snapshot, commands and events.
//!
//! The app shell starts the core when the app launches, whether or not a web view is open, and
//! forwards its events to the web view when there is one. Background work, such as radio
//! messages while the phone is locked, needs the core to run without a web view.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use marmot_app::AppError;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::interface::{
    Account, Chat, CommandError, Contact, CoreEvent, DeliveryState, Direction, Invitation, Message,
    Relay, RelayOperator, RelayStatus, Snapshot, Transport,
};
use crate::invite;
use crate::messaging::{
    DirectChat, Messaging, MessagingError, MessagingOptions, NewAccount, PROJECT_RELAYS, RelayLink,
    RelayLinks, TextMessage, canonical_relay,
};
use crate::radio::RadioKeys;
use crate::reach::Reachability;

/// How many events a listener may fall behind before it misses some.
const EVENT_BUFFER: usize = 64;
/// The shortest password for an exported key, as the design asks.
pub const MIN_EXPORT_PASSWORD_CHARS: usize = 12;
/// The longest profile name, in characters (#93).
pub const MAX_NAME_CHARS: usize = 64;
/// The longest about text, in characters (#93).
pub const MAX_ABOUT_CHARS: usize = 280;
/// How often the core fetches its contacts' profiles again, for their new names (#93).
const CONTACT_REFRESH: Duration = Duration::from_secs(5 * 60);

/// The protocol core. Clones share one core.
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

struct Inner {
    messaging: Messaging,
    state: Mutex<Snapshot>,
    events: broadcast::Sender<CoreEvent>,
    /// Commands that create or delete the account run one at a time.
    account_commands: tokio::sync::Mutex<()>,
    /// Follow the account's chats, invitations, messages and relays while there is an account.
    watchers: Mutex<Vec<JoinHandle<()>>>,
    /// Which chats can reach their relays (#43). Whoever holds both locks takes `state` first.
    reach: Mutex<Reachability>,
}

impl Core {
    /// Opens the core's stores and starts messaging. The snapshot then holds the account that is
    /// already on this device, if any, with its chats and their latest messages; chats whose
    /// contacts' names aren't known yet follow as events. Must run inside a Tokio runtime.
    pub async fn start(options: MessagingOptions) -> Result<Self, MessagingError> {
        let messaging = Messaging::open(options).await?;
        // The proof of concept has one account per device.
        let account = match messaging.accounts()?.into_iter().next() {
            Some(existing) => Some(account_view(
                &messaging,
                existing.public_key_hex,
                existing.name.unwrap_or_default(),
            )?),
            None => None,
        };
        log::info!(
            "core started {} an account",
            if account.is_some() { "with" } else { "without" }
        );
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let core = Self {
            inner: Arc::new(Inner {
                messaging,
                state: Mutex::new(Snapshot {
                    account: account.clone(),
                    ..Snapshot::default()
                }),
                events,
                account_commands: tokio::sync::Mutex::new(()),
                watchers: Mutex::new(Vec::new()),
                reach: Mutex::new(Reachability::default()),
            }),
        };
        if let Some(account) = account {
            core.follow(&account.public_key).await;
        }
        Ok(core)
    }

    /// Everything an interface shows, as it is now.
    pub fn snapshot(&self) -> Snapshot {
        self.state().clone()
    }

    /// Follows the core's events from now on. A listener that falls more than 64 events behind
    /// misses some, and should load a new snapshot.
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.inner.events.subscribe()
    }

    /// First launch: creates the account's key and `name` as its profile name. Emits
    /// [`CoreEvent::AccountCreated`].
    ///
    /// The key stays in the core: it goes to the platform's key store, and only the public key
    /// leaves.
    pub async fn create_account(&self, name: &str) -> Result<Account, CommandError> {
        let name = checked_name(name)?;
        let _one_at_a_time = self.inner.account_commands.lock().await;
        if self.state().account.is_some() {
            return Err(CommandError::new("This device already has an account."));
        }

        // MDK needs relays to set an account up, though not reachable ones: publishing waits.
        let created =
            self.inner
                .messaging
                .create_account(name)
                .await
                .map_err(|error| match error {
                    MessagingError::Mdk(AppError::MissingDefaultRelays) => {
                        CommandError::new("This build has no relays set up.")
                    }
                    error => failed("create the account", error),
                })?;
        let account = account_view(
            &self.inner.messaging,
            created.public_key_hex,
            name.to_owned(),
        )
        .map_err(|error| failed("create the account", error))?;

        log::info!("account created");
        self.state().account = Some(account.clone());
        self.emit(CoreEvent::AccountCreated {
            account: account.clone(),
        });
        self.follow(&account.public_key).await;
        Ok(account)
    }

    /// Changes the profile's name and about, and publishes them (#93). Emits
    /// [`CoreEvent::AccountChanged`]. The photo and anything other apps put in the profile
    /// stay as they are.
    pub async fn update_profile(&self, name: &str, about: &str) -> Result<Account, CommandError> {
        let name = checked_name(name)?;
        let about = about.trim();
        if about.chars().count() > MAX_ABOUT_CHARS {
            return Err(CommandError::new(format!(
                "About can have up to {MAX_ABOUT_CHARS} characters."
            )));
        }
        let _one_at_a_time = self.inner.account_commands.lock().await;
        let account = self.account()?;
        if account.name == name && account.about == about {
            return Ok(account);
        }
        self.inner
            .messaging
            .update_profile(&account.public_key, name, about)
            .await
            .map_err(|error| {
                // MDK publishes the profile at once, or not at all.
                if self.any_relay_connected() {
                    failed("update your profile", error)
                } else {
                    log::warn!("could not update the profile without relays: {error}");
                    CommandError::new("Could not reach the relays. Try again when you are online.")
                }
            })?;
        log::info!("profile updated");

        let mut state = self.state();
        let Some(current) = state.account.as_mut() else {
            return Err(CommandError::new("Create an account first."));
        };
        current.name = name.to_owned();
        current.about = about.to_owned();
        let account = current.clone();
        self.emit(CoreEvent::AccountChanged {
            account: account.clone(),
        });
        Ok(account)
    }

    /// Fetches the contacts' profiles again, and emits [`CoreEvent::ContactChanged`] for each
    /// contact whose name changed (#93). The core does this as it starts following the account
    /// and every few minutes after; MDK doesn't follow other people's profiles by itself.
    pub async fn refresh_contacts(&self) {
        let contacts: std::collections::BTreeMap<String, String> = {
            let state = self.state();
            state
                .chats
                .iter()
                .map(|chat| &chat.contact)
                .chain(state.invitations.iter().map(|invitation| &invitation.from))
                .map(|contact| (contact.public_key.clone(), contact.name.clone()))
                .collect()
        };
        for (public_key, name) in contacts {
            let Some(fetched) = self.inner.messaging.fetch_name(&public_key, &[]).await else {
                continue;
            };
            if fetched != name {
                self.rename_contact(&public_key, &fetched);
            }
        }
    }

    fn any_relay_connected(&self) -> bool {
        self.reach()
            .relays(Instant::now())
            .iter()
            .any(|(_, status)| *status == RelayStatus::Connected)
    }

    /// Gives the contact with `public_key` a new name in their chats and invitations.
    fn rename_contact(&self, public_key: &str, name: &str) {
        let mut state = self.state();
        let state = &mut *state;
        let mut changed = false;
        let contacts = state.chats.iter_mut().map(|chat| &mut chat.contact).chain(
            state
                .invitations
                .iter_mut()
                .map(|invitation| &mut invitation.from),
        );
        for contact in contacts {
            if contact.public_key == public_key && contact.name != name {
                contact.name = name.to_owned();
                changed = true;
            }
        }
        if changed {
            log::info!("a contact's name changed");
            self.emit(CoreEvent::ContactChanged {
                contact: Contact {
                    public_key: public_key.to_owned(),
                    name: name.to_owned(),
                },
            });
        }
    }

    /// Removes the account and its data from this device, including its key. Emits
    /// [`CoreEvent::AccountDeleted`].
    pub async fn delete_account(&self) -> Result<(), CommandError> {
        let _one_at_a_time = self.inner.account_commands.lock().await;
        let Some(account) = self.state().account.clone() else {
            return Err(CommandError::new("There is no account on this device."));
        };

        // Stop first, so that nothing of the deleted account arrives afterwards.
        self.stop_following();
        if let Err(error) = self
            .inner
            .messaging
            .delete_account(&account.public_key)
            .await
        {
            self.follow(&account.public_key).await;
            return Err(failed("delete the account", error));
        }

        log::info!("account deleted");
        *self.state() = Snapshot::default();
        *self.reach() = Reachability::default();
        self.emit(CoreEvent::AccountDeleted);
        Ok(())
    }

    /// Starts a direct chat with the person whose invite link, QR code or key this is (#19).
    /// Emits [`CoreEvent::ChatAdded`].
    ///
    /// MDK fetches their relay lists and KeyPackage and sends them an encrypted invitation; the
    /// chat opens here at once. With someone who is a contact already, this returns their chat.
    /// If they have invited the user already, this accepts that invitation instead.
    pub async fn add_contact(&self, invite_link: &str) -> Result<Chat, CommandError> {
        let account = self.account()?;
        let invite =
            invite::parse(invite_link).map_err(|error| CommandError::new(error.to_string()))?;
        if invite.public_key_hex == account.public_key {
            return Err(CommandError::new("That is your own invite link."));
        }
        if let Some(chat) = self.chat_with(&invite.public_key_hex) {
            return Ok(chat);
        }
        if let Some(invitation_id) = self.invitation_from(&invite.public_key_hex) {
            return self.accept_invitation(&invitation_id).await;
        }

        let chat_id = self
            .inner
            .messaging
            .start_direct_chat(&account.public_key, &invite.public_key_hex, &invite.relays)
            .await
            .map_err(|error| {
                if error.is_contact_unreachable() {
                    log::info!("could not find the contact's account: {error}");
                    CommandError::new("Could not find this person's account. Try again later.")
                } else {
                    failed("start the chat", error)
                }
            })?;
        log::info!("direct chat started");
        let name = self
            .inner
            .messaging
            .contact_name(&invite.public_key_hex, &invite.relays)
            .await;
        let started = DirectChat {
            id: chat_id,
            peer_hex: invite.public_key_hex,
            invitation: false,
            active: true,
            relays: Vec::new(),
        };
        self.apply(&started, name, now_ms());
        self.chat(&started.id)
            .ok_or_else(|| failed("start the chat", "the chat is missing after starting it"))
    }

    /// Accepts an invitation to a direct chat (#19). Emits [`CoreEvent::InvitationRemoved`],
    /// then [`CoreEvent::ChatAdded`] for the same id, with the messages that came so far.
    pub async fn accept_invitation(&self, invitation_id: &str) -> Result<Chat, CommandError> {
        let account = self.account()?;
        let invitation = self
            .state()
            .invitations
            .iter()
            .find(|invitation| invitation.id == invitation_id)
            .cloned()
            .ok_or_else(|| CommandError::new("This request is no longer available."))?;

        self.inner
            .messaging
            .accept_direct_chat(&account.public_key, &invitation.id)
            .await
            .map_err(|error| failed("accept the request", error))?;
        log::info!("invitation accepted");
        let accepted = DirectChat {
            id: invitation.id.clone(),
            peer_hex: invitation.from.public_key.clone(),
            invitation: false,
            active: true,
            relays: Vec::new(),
        };
        self.apply(&accepted, invitation.from.name, invitation.received_at);
        self.chat(&invitation.id)
            .ok_or_else(|| failed("accept the request", "the chat is missing after accepting"))
    }

    /// Sends `text` in the chat `chat_id` (#20). Emits [`CoreEvent::MessageAdded`], and
    /// [`CoreEvent::DeliveryChanged`] once a relay has acknowledged the message.
    ///
    /// The message is stored before this returns. While no relay can be reached, MDK keeps it
    /// and tries again, also after a restart; until then it is `waiting`.
    pub async fn send_message(&self, chat_id: &str, text: &str) -> Result<Message, CommandError> {
        let account = self.account()?;
        let text = text.trim();
        if text.is_empty() {
            return Err(CommandError::new("Write a message first."));
        }
        if self.chat(chat_id).is_none() {
            return Err(CommandError::new("This chat no longer exists."));
        }

        let message_id = self
            .inner
            .messaging
            .send_text(&account.public_key, chat_id, text)
            .await
            .map_err(|error| failed("send the message", error))?;
        // MDK has the message now. Its watcher sees it too; whichever is first adds it.
        let sent = self
            .inner
            .messaging
            .chat_message(&account.public_key, chat_id, &message_id)
            .unwrap_or_else(|| TextMessage {
                chat_id: chat_id.to_owned(),
                id: message_id.clone(),
                outgoing: true,
                text: text.to_owned(),
                sent_at: now_ms() / 1000 * 1000,
                acknowledged: false,
            });
        self.apply_message(&sent);
        self.chat(chat_id)
            .and_then(|chat| {
                chat.messages
                    .into_iter()
                    .find(|message| message.id == message_id)
            })
            .ok_or_else(|| failed("send the message", "the message is missing after sending"))
    }

    /// The keys for radio messages in the chat `chat_id`, in its current MLS epoch (#37). For the
    /// radio transport inside the core (#39), not for an interface: the keys never leave the core.
    pub async fn radio_keys(&self, chat_id: &str) -> Result<RadioKeys, CommandError> {
        let account = self.account()?;
        let chat = self
            .chat(chat_id)
            .ok_or_else(|| CommandError::new("This chat no longer exists."))?;
        let secret = self
            .inner
            .messaging
            .chat_secret(&account.public_key, chat_id)
            .await
            .map_err(|error| failed("derive the radio keys", error))?;
        RadioKeys::derive(
            &secret.secret,
            secret.epoch,
            &account.public_key,
            &chat.contact.public_key,
        )
        .map_err(|error| failed("derive the radio keys", error))
    }

    /// Updates the account's own key in the chat `chat_id`. Once the relays confirm it, the
    /// chat is in a new MLS epoch, with new radio keys. MDK does this by itself every 24 to 36
    /// days.
    pub async fn update_own_key(&self, chat_id: &str) -> Result<(), CommandError> {
        let account = self.account()?;
        if self.chat(chat_id).is_none() {
            return Err(CommandError::new("This chat no longer exists."));
        }
        self.inner
            .messaging
            .update_own_key(&account.public_key, chat_id)
            .await
            .map_err(|error| failed("update the key", error))
    }

    /// Exports the account key (#86; spec v0.2, Key export). With `password`, a NIP-49
    /// `ncryptsec1…` that other Nostr apps and signers open with it; without one, the plain
    /// `nsec1…`, as the user's explicit choice.
    ///
    /// `confirm_owner` asks for the device unlock. It runs once the request is otherwise
    /// valid, so nobody unlocks only to hear that the password is too short, and an error from
    /// it stops the export. The key is logged nowhere: keep the result only while it is shown.
    pub async fn export_account_key<F, C>(
        &self,
        password: Option<&str>,
        confirm_owner: C,
    ) -> Result<Zeroizing<String>, CommandError>
    where
        C: FnOnce() -> F,
        F: Future<Output = Result<(), CommandError>>,
    {
        let account = self.account()?;
        if password.is_some_and(|password| password.chars().count() < MIN_EXPORT_PASSWORD_CHARS) {
            return Err(CommandError::new(format!(
                "Use a password of at least {MIN_EXPORT_PASSWORD_CHARS} characters."
            )));
        }
        confirm_owner().await?;

        let core = self.clone();
        let password = password.map(|password| Zeroizing::new(password.to_owned()));
        let protected = password.is_some();
        // NIP-49's scrypt takes a second and a lot of memory: not on the async threads.
        let exported = tokio::task::spawn_blocking(move || {
            core.inner.messaging.export_key(
                &account.public_key,
                password.as_ref().map(|password| password.as_str()),
            )
        })
        .await
        .map_err(|error| failed("export the key", error))?
        .map_err(|error| failed("export the key", error))?;
        log::info!(
            "account key exported, {}",
            if protected { "protected" } else { "plain" }
        );
        Ok(exported)
    }

    /// Waits until the relays have the account's relay lists, profile and KeyPackage, so that
    /// others can invite it.
    pub async fn wait_until_published(&self, timeout: Duration) -> Result<(), CommandError> {
        let account = self.account()?;
        self.inner
            .messaging
            .wait_until_published(
                &NewAccount {
                    public_key_hex: account.public_key,
                },
                timeout,
            )
            .await
            .map_err(|error| failed("publish the account", error))
    }

    /// Stops messaging and closes the stores. The core cannot be used afterwards.
    pub async fn close(&self) -> Result<(), MessagingError> {
        self.stop_following();
        self.inner.messaging.close().await
    }

    /// Puts the account's chats and invitations into the snapshot, with their latest messages,
    /// and keeps them up to date. Chats whose contacts' names are known go in now; the rest
    /// arrive as events once their names are. Follows the relays too, for each chat's reach.
    async fn follow(&self, account_hex: &str) {
        // Messages first: a message that arrives while the chats are being read is then either
        // in a chat's history already or among the messages followed from here.
        let messages = self.inner.messaging.follow_messages(account_hex).await;
        let chats = self.inner.messaging.follow_direct_chats(account_hex).await;
        let (mut messages, mut chats) = match (messages, chats) {
            (Ok(messages), Ok(chats)) => (messages, chats),
            (Err(error), _) | (_, Err(error)) => {
                log::warn!("could not follow the account's chats and messages: {error}");
                return;
            }
        };
        // The relays before the chats, so that the chats start out with their reach. Without
        // them, every chat keeps the internet.
        let relays = match self.inner.messaging.follow_relays(account_hex).await {
            Ok(relays) => {
                *self.reach() = Reachability::new(&relays.current(), Instant::now());
                Some(relays)
            }
            Err(error) => {
                log::warn!("could not follow the relays: {error}");
                None
            }
        };
        let mut unnamed = Vec::new();
        for chat in chats.current() {
            self.reach().set_chat_relays(&chat.id, &chat.relays);
            match self.inner.messaging.known_name(&chat.peer_hex) {
                Some(name) => {
                    let started = self.started_at(&chat.id);
                    self.apply(&chat, name, started);
                }
                None => unnamed.push(chat),
            }
        }
        // The relays go into the snapshot now; their watcher keeps them current.
        self.refresh_reach(Instant::now());

        // The watchers hold the core weakly, so dropping the core ends them.
        let core = Arc::downgrade(&self.inner);
        let chat_watcher = tokio::spawn(async move {
            for chat in unnamed {
                let Some(core) = upgrade(&core) else { return };
                core.follow_chat(chat).await;
            }
            while let Some(chat) = chats.next().await {
                let Some(core) = upgrade(&core) else { return };
                core.follow_chat(chat).await;
            }
        });
        let core = Arc::downgrade(&self.inner);
        let message_watcher = tokio::spawn(async move {
            while let Some(message) = messages.next().await {
                let Some(core) = upgrade(&core) else { return };
                core.apply_message(&message);
            }
        });
        let core = Arc::downgrade(&self.inner);
        let contact_watcher = tokio::spawn(async move {
            loop {
                let Some(core) = upgrade(&core) else { return };
                core.refresh_contacts().await;
                drop(core);
                tokio::time::sleep(CONTACT_REFRESH).await;
            }
        });
        let mut watchers = vec![chat_watcher, message_watcher, contact_watcher];
        if let Some(relays) = relays {
            watchers.push(tokio::spawn(watch_relays(
                Arc::downgrade(&self.inner),
                relays,
            )));
        }
        let previous = std::mem::replace(&mut *self.watchers(), watchers);
        for watcher in previous {
            watcher.abort();
        }
    }

    fn stop_following(&self) {
        for watcher in self.watchers().drain(..) {
            watcher.abort();
        }
    }

    /// Brings one chat from MDK into the snapshot, first finding the contact's name if needed.
    async fn follow_chat(&self, chat: DirectChat) {
        // The relays the chat goes through may decide its reach.
        self.reach().set_chat_relays(&chat.id, &chat.relays);
        self.refresh_reach(Instant::now());
        if self.reflects(&chat) {
            return;
        }
        let name = self.inner.messaging.contact_name(&chat.peer_hex, &[]).await;
        let started = self.started_at(&chat.id);
        self.apply(&chat, name, started);
    }

    /// Whether the snapshot shows `chat` as it is already.
    fn reflects(&self, chat: &DirectChat) -> bool {
        let state = self.state();
        let is_chat = state.chats.iter().any(|known| known.id == chat.id);
        let is_invitation = state
            .invitations
            .iter()
            .any(|invitation| invitation.id == chat.id);
        if !chat.active {
            !is_invitation
        } else if chat.invitation {
            is_chat || is_invitation
        } else {
            is_chat
        }
    }

    /// Records `chat` in the snapshot and emits what changed. The commands and the watchers all
    /// come through here, under the state lock, so each change is emitted once.
    ///
    /// A chat or invitation that is new to the snapshot gets its history here, under the same
    /// lock: a message stored earlier is in it, and one stored later reaches
    /// [`Self::apply_message`] after the chat is in the snapshot.
    fn apply(&self, chat: &DirectChat, contact_name: String, started_at: u64) {
        let mut state = self.state();
        let Some(account_hex) = state
            .account
            .as_ref()
            .map(|account| account.public_key.clone())
        else {
            return;
        };
        let is_chat = state.chats.iter().any(|known| known.id == chat.id);
        let invitation = state
            .invitations
            .iter()
            .position(|invitation| invitation.id == chat.id);
        let contact = || Contact {
            public_key: chat.peer_hex.clone(),
            name: contact_name.clone(),
        };

        let mut events = Vec::new();
        if !chat.active {
            // Declined elsewhere, withdrawn or left. The interface has no event for a chat that
            // ends, so only invitations go away.
            if let Some(index) = invitation {
                state.invitations.remove(index);
                events.push(CoreEvent::InvitationRemoved {
                    invitation_id: chat.id.clone(),
                });
            }
        } else if chat.invitation {
            if !is_chat && invitation.is_none() {
                let invitation = Invitation {
                    id: chat.id.clone(),
                    from: contact(),
                    received_at: started_at,
                    messages: self.history(&account_hex, &chat.id),
                };
                state.invitations.push(invitation.clone());
                events.push(CoreEvent::InvitationReceived { invitation });
            }
        } else if !is_chat {
            let contact = match invitation {
                Some(index) => {
                    let accepted = state.invitations.remove(index);
                    events.push(CoreEvent::InvitationRemoved {
                        invitation_id: chat.id.clone(),
                    });
                    accepted.from
                }
                None => contact(),
            };
            let added = Chat {
                id: chat.id.clone(),
                contact,
                messages: self.history(&account_hex, &chat.id),
                reach: self.reach().reach(&chat.id, Instant::now()),
            };
            state.chats.push(added.clone());
            events.push(CoreEvent::ChatAdded { chat: added });
        }
        for event in events {
            self.emit(event);
        }
    }

    /// Records a message that was added or changed, and emits what changed: a new message, or
    /// an outgoing one that a relay has now acknowledged. A message for a chat the snapshot
    /// doesn't have yet is in that chat's history when it arrives.
    fn apply_message(&self, message: &TextMessage) {
        let view = message_view(message);
        let mut guard = self.state();
        let state = &mut *guard;
        let messages = if let Some(chat) = state.chats.iter_mut().find(|c| c.id == message.chat_id)
        {
            &mut chat.messages
        } else if let Some(invitation) = state
            .invitations
            .iter_mut()
            .find(|invitation| invitation.id == message.chat_id)
        {
            // Messages that come before the user accepts the invitation.
            &mut invitation.messages
        } else {
            return;
        };

        let event = match messages.iter_mut().find(|known| known.id == view.id) {
            None => {
                messages.push(view.clone());
                CoreEvent::MessageAdded {
                    chat_id: message.chat_id.clone(),
                    message: view,
                }
            }
            // Delivery only moves forward.
            Some(known)
                if known.delivery == Some(DeliveryState::Waiting)
                    && view.delivery == Some(DeliveryState::Sent) =>
            {
                known.delivery = view.delivery;
                CoreEvent::DeliveryChanged {
                    chat_id: message.chat_id.clone(),
                    message_id: view.id,
                    delivery: DeliveryState::Sent,
                }
            }
            Some(_) => return,
        };
        self.emit(event);
    }

    /// Takes the relays' connections as they are now, and updates each chat's reach with them.
    /// Returns when to look again.
    fn relays_changed(&self, links: &[RelayLink]) -> Option<Instant> {
        let now = Instant::now();
        if self.reach().update(links, now) {
            log::info!("relays are back after an outage");
            // Waiting messages go now, not at their next retry. That can take a while, and the
            // chats' reach shouldn't wait for it.
            let core = self.clone();
            tokio::spawn(async move { core.inner.messaging.connectivity_restored().await });
        }
        self.refresh_reach(now)
    }

    /// Sets each chat's reach and each relay's status as they are at `now`, and emits
    /// [`CoreEvent::ReachChanged`] and [`CoreEvent::RelaysChanged`] for what changed. Returns
    /// when a closed connection next makes its relay count as gone.
    fn refresh_reach(&self, now: Instant) -> Option<Instant> {
        let mut state = self.state();
        let reachability = self.reach();
        let relays = relay_views(reachability.relays(now), PROJECT_RELAYS);
        if state.account.is_some() && state.relays != relays {
            state.relays = relays.clone();
            self.emit(CoreEvent::RelaysChanged { relays });
        }
        for chat in &mut state.chats {
            let reach = reachability.reach(&chat.id, now);
            if chat.reach != reach {
                chat.reach = reach;
                log::info!("a chat's reach is now {reach:?}");
                self.emit(CoreEvent::ReachChanged {
                    chat_id: chat.id.clone(),
                    reach,
                });
            }
        }
        reachability.next_change(now)
    }

    /// The latest messages of a chat or invitation.
    fn history(&self, account_hex: &str, chat_id: &str) -> Vec<Message> {
        match self.inner.messaging.chat_messages(account_hex, chat_id) {
            Ok(messages) => messages.iter().map(message_view).collect(),
            Err(error) => {
                log::warn!("could not read a chat's messages: {error}");
                Vec::new()
            }
        }
    }

    fn account(&self) -> Result<Account, CommandError> {
        self.state()
            .account
            .clone()
            .ok_or_else(|| CommandError::new("Create an account first."))
    }

    fn chat(&self, id: &str) -> Option<Chat> {
        self.state()
            .chats
            .iter()
            .find(|chat| chat.id == id)
            .cloned()
    }

    fn chat_with(&self, public_key: &str) -> Option<Chat> {
        self.state()
            .chats
            .iter()
            .find(|chat| chat.contact.public_key == public_key)
            .cloned()
    }

    fn invitation_from(&self, public_key: &str) -> Option<String> {
        self.state()
            .invitations
            .iter()
            .find(|invitation| invitation.from.public_key == public_key)
            .map(|invitation| invitation.id.clone())
    }

    /// When this device first saw the chat, or now.
    fn started_at(&self, chat_id: &str) -> u64 {
        let Some(account) = self.state().account.clone() else {
            return now_ms();
        };
        self.inner
            .messaging
            .chat_started_at(&account.public_key, chat_id)
            .unwrap_or_else(now_ms)
    }

    fn state(&self) -> MutexGuard<'_, Snapshot> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn watchers(&self) -> MutexGuard<'_, Vec<JoinHandle<()>>> {
        self.inner
            .watchers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn reach(&self) -> MutexGuard<'_, Reachability> {
        self.inner
            .reach
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn emit(&self, event: CoreEvent) {
        // Having no listener is fine: the core runs without a web view.
        let _ = self.inner.events.send(event);
    }
}

fn upgrade(core: &Weak<Inner>) -> Option<Core> {
    core.upgrade().map(|inner| Core { inner })
}

/// Follows the account's relays, and updates each chat's reach as relays come and go and as
/// closed connections run out their time.
async fn watch_relays(core: Weak<Inner>, relays: RelayLinks) {
    let mut next = upgrade(&core).and_then(|core| core.refresh_reach(Instant::now()));
    loop {
        let time_is_up = async {
            match next {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        let changed = tokio::select! {
            links = relays.next() => match links {
                Some(links) => Some(links),
                None => return,
            },
            () = time_is_up => None,
        };
        let Some(core) = upgrade(&core) else { return };
        next = match changed {
            Some(links) => core.relays_changed(&links),
            None => core.refresh_reach(Instant::now()),
        };
    }
}

/// The relays as an interface shows them, with who runs each: the `project`'s first, then the
/// others. The sort keeps each group in the order it came, which is by address.
fn relay_views(relays: Vec<(String, RelayStatus)>, project: &[&str]) -> Vec<Relay> {
    let mut relays: Vec<Relay> = relays
        .into_iter()
        .map(|(url, status)| Relay {
            operator: if project.iter().any(|relay| canonical_relay(relay) == url) {
                RelayOperator::PeoplesChat
            } else {
                RelayOperator::Independent
            },
            url,
            status,
        })
        .collect();
    relays.sort_by_key(|relay| relay.operator != RelayOperator::PeoplesChat);
    relays
}

/// A profile name as given, trimmed, or why it can't be one.
fn checked_name(name: &str) -> Result<&str, CommandError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CommandError::new("Enter a name."));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CommandError::new(format!(
            "Your name can have up to {MAX_NAME_CHARS} characters."
        )));
    }
    Ok(name)
}

/// The account as an interface sees it. The invite link carries the relays new accounts
/// publish to as hints (#19).
fn account_view(
    messaging: &Messaging,
    public_key_hex: String,
    name: String,
) -> Result<Account, MessagingError> {
    let invite_link = invite::invite_link(&public_key_hex, messaging.relays())?;
    Ok(Account {
        name,
        about: messaging.own_about(&public_key_hex),
        invite_link,
        public_key: public_key_hex,
    })
}

/// A message as an interface sees it. Internet only, until the radio (#40).
fn message_view(message: &TextMessage) -> Message {
    Message {
        id: message.id.clone(),
        direction: if message.outgoing {
            Direction::Out
        } else {
            Direction::In
        },
        text: message.text.clone(),
        sent_at: message.sent_at,
        transport: Transport::Internet,
        delivery: message.outgoing.then_some(if message.acknowledged {
            DeliveryState::Sent
        } else {
            DeliveryState::Waiting
        }),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis().try_into().unwrap_or(u64::MAX))
}

/// Logs what went wrong and returns an error an interface can show.
fn failed(action: &str, error: impl std::fmt::Display) -> CommandError {
    log::warn!("could not {action}: {error}");
    CommandError::new(format!("Could not {action}. Try again."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_projects_relays_come_first_and_say_who_runs_them() {
        let relays = vec![
            ("wss://a.example".to_owned(), RelayStatus::Connected),
            (
                "wss://relay.peoples.chat".to_owned(),
                RelayStatus::Unreachable,
            ),
            ("wss://z.example".to_owned(), RelayStatus::Connecting),
        ];
        // However the project's list spells an address.
        let views = relay_views(relays, &["wss://Relay.Peoples.Chat/"]);
        let shown: Vec<_> = views
            .iter()
            .map(|relay| (relay.url.as_str(), relay.operator, relay.status))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "wss://relay.peoples.chat",
                    RelayOperator::PeoplesChat,
                    RelayStatus::Unreachable
                ),
                (
                    "wss://a.example",
                    RelayOperator::Independent,
                    RelayStatus::Connected
                ),
                (
                    "wss://z.example",
                    RelayOperator::Independent,
                    RelayStatus::Connecting
                ),
            ]
        );
    }
}
