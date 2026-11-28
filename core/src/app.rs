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
    Account, Chat, ChatState, CommandError, Contact, CoreEvent, DeliveryState, Direction,
    Invitation, Message, RadioIssue, Reach, Relay, RelayOperator, RelayStatus, Sender, SentInvite,
    Snapshot, Transport,
};
use crate::invite;
use crate::invites::{InviteRecord, Invites};
use crate::known_chats::KnownChats;
use crate::messaging::{
    ChatForm, ChatItem, DirectChat, EpochChange, InviteToken, Messaging, MessagingError,
    MessagingOptions, NewAccount, PROJECT_RELAYS, RelayLink, RelayLinks, TextMessage,
    UndeliveredInvitation, canonical_relay,
};
use crate::outbox::Outbox;
use crate::photo::{self, AVATAR_SIDE, Avatars, UPLOAD_SIDE};
use crate::radio_store::{RadioMessage, RadioStore};
use crate::reach::{Reachability, radio_reach};
use crate::transport::{Route, route};

mod expiry;
mod pairing;
mod radio;
mod undelivered;
mod waiting;

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
    /// Wakes the relays' watcher to look again at when a chat's reach changes next, as when a
    /// radio was heard (#44).
    reach_wake: Arc<tokio::sync::Notify>,
    /// Wakes the watcher of disappearing messages (#124).
    expiry: Arc<expiry::ExpiryWake>,
    /// Avatars made from profile photos (#94).
    avatars: Avatars,
    /// The invitations the user sent (#95).
    invites: Invites,
    /// The direct chats the core has shown, and with whom, for when they stop being direct
    /// (#125).
    known_chats: KnownChats,
    /// What the core keeps about the radio (#40).
    radio_store: RadioStore,
    /// The radio the core uses, and the chats' radio keys.
    radio: radio::RadioParts,
    /// Messages waiting for a path (#46).
    outbox: Outbox,
    /// Chats whose waiting messages are being sent.
    flushing: Mutex<std::collections::HashSet<String>>,
    /// Invitations the user sent that haven't reached the contact yet, as MDK last listed them
    /// (#126). Whoever holds both locks takes `state` first.
    undelivered: Mutex<Vec<UndeliveredInvitation>>,
    /// Wakes the invitations' watcher to send undelivered ones again: relays are back.
    invitation_wake: Arc<tokio::sync::Notify>,
}

impl Core {
    /// Opens the core's stores and starts messaging. The snapshot then holds the account that is
    /// already on this device, if any, with its chats and their latest messages; chats whose
    /// contacts' names aren't known yet follow as events. Must run inside a Tokio runtime.
    pub async fn start(options: MessagingOptions) -> Result<Self, MessagingError> {
        let avatars = Avatars::new(&options.data_dir);
        let invites = Invites::open(&options.data_dir);
        let known_chats = KnownChats::open(&options.data_dir);
        let radio_store = RadioStore::open(&options.data_dir);
        // Before the chats load, so that they show radio messages that timed out meanwhile (#41).
        let awaited = radio_store.after_restart(now_ms()).unwrap_or_else(|error| {
            log::warn!("could not check the radio confirmations: {error}");
            Vec::new()
        });
        let outbox = Outbox::open(&options.data_dir);
        let messaging = Messaging::open(options).await?;
        // The proof of concept has one account per device.
        let account = match messaging.accounts()?.into_iter().next() {
            Some(existing) => Some(account_view(
                &messaging,
                &avatars,
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
                    invites: account
                        .as_ref()
                        .map(|account| invite_views(account, &invites.all()))
                        .unwrap_or_default(),
                    account: account.clone(),
                    ..Snapshot::default()
                }),
                events,
                account_commands: tokio::sync::Mutex::new(()),
                watchers: Mutex::new(Vec::new()),
                reach: Mutex::new(Reachability::default()),
                reach_wake: Arc::new(tokio::sync::Notify::new()),
                expiry: Arc::default(),
                avatars,
                invites,
                known_chats,
                radio_store,
                radio: radio::RadioParts::default(),
                outbox,
                flushing: Mutex::new(std::collections::HashSet::new()),
                undelivered: Mutex::new(Vec::new()),
                invitation_wake: Arc::new(tokio::sync::Notify::new()),
            }),
        };
        // The switch, and the paired radio until the shell gives the core its ports (#24).
        core.radio_changed();
        if let Some(account) = account {
            core.follow(&account.public_key).await;
        }
        core.await_after_restart(awaited);
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
            &self.inner.avatars,
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
            .map_err(|error| self.profile_not_published("update your profile", error))?;
        log::info!("profile updated");
        self.change_account(|account| {
            account.name = name.to_owned();
            account.about = about.to_owned();
        })
    }

    /// Sets the profile photo (#94). The core turns `photo` upright, cuts it to a centred
    /// square, shrinks it and saves it again as JPEG, which drops its metadata, such as where it
    /// was taken. It then uploads it to the media server and publishes its address in the
    /// profile. Emits [`CoreEvent::AccountChanged`].
    pub async fn set_profile_photo(&self, photo: &[u8]) -> Result<Account, CommandError> {
        self.account()?;
        let upload = photo::square_jpeg(photo, UPLOAD_SIDE).map_err(|error| {
            log::info!("a photo was refused: {error}");
            CommandError::new("This photo can't be used. Try another one.")
        })?;
        let avatar = photo::square_jpeg(&upload, AVATAR_SIDE)
            .map_err(|error| failed("set your photo", error))?;
        let _one_at_a_time = self.inner.account_commands.lock().await;
        let account = self.account()?;
        let url = self
            .inner
            .messaging
            .upload_photo(&account.public_key, upload)
            .await
            .map_err(|error| failed("upload your photo", error))?;
        self.inner
            .messaging
            .set_picture(&account.public_key, Some(&url))
            .await
            .map_err(|error| self.profile_not_published("set your photo", error))?;
        log::info!("profile photo set");
        if let Err(error) = self.inner.avatars.put(&url, &avatar) {
            log::warn!("could not keep the avatar: {error}");
        }
        self.change_account(|account| account.photo = Some(photo::data_url(&avatar)))
    }

    /// Removes the profile photo (#94). Emits [`CoreEvent::AccountChanged`]. The photo stays on
    /// the media server: its address is just no longer in the profile.
    pub async fn remove_profile_photo(&self) -> Result<Account, CommandError> {
        let _one_at_a_time = self.inner.account_commands.lock().await;
        let account = self.account()?;
        if account.photo.is_none()
            && self
                .inner
                .messaging
                .known_picture(&account.public_key)
                .is_none()
        {
            return Ok(account);
        }
        self.inner
            .messaging
            .set_picture(&account.public_key, None)
            .await
            .map_err(|error| self.profile_not_published("remove your photo", error))?;
        log::info!("profile photo removed");
        self.change_account(|account| account.photo = None)
    }

    /// Applies `change` to the account in the snapshot and emits [`CoreEvent::AccountChanged`].
    fn change_account(&self, change: impl FnOnce(&mut Account)) -> Result<Account, CommandError> {
        let mut state = self.state();
        let Some(current) = state.account.as_mut() else {
            return Err(CommandError::new("Create an account first."));
        };
        change(current);
        let account = current.clone();
        self.emit(CoreEvent::AccountChanged {
            account: account.clone(),
        });
        Ok(account)
    }

    /// Why a profile wasn't published. MDK publishes at once, or not at all.
    fn profile_not_published(&self, action: &str, error: MessagingError) -> CommandError {
        if self.any_relay_connected() {
            failed(action, error)
        } else {
            log::warn!("could not {action} without relays: {error}");
            CommandError::new("Could not reach the relays. Try again when you are online.")
        }
    }

    /// Fetches the contacts' profiles again, and emits [`CoreEvent::ContactChanged`] for each
    /// contact whose name or photo changed (#93, #94). The core does this as it starts following
    /// the account and every few minutes after; MDK doesn't follow other people's profiles by
    /// itself.
    pub async fn refresh_contacts(&self) {
        let contacts: std::collections::BTreeSet<String> = {
            let state = self.state();
            state
                .chats
                .iter()
                .map(|chat| chat.contact.public_key.clone())
                .chain(
                    state
                        .invitations
                        .iter()
                        .map(|invitation| invitation.from.public_key.clone()),
                )
                .collect()
        };
        for public_key in contacts {
            let Some(profile) = self.inner.messaging.fetch_profile(&public_key, &[]).await else {
                continue;
            };
            let photo = match &profile.picture {
                Some(url) => self.avatar_for(url).await,
                None => None,
            };
            self.update_contact(&public_key, profile.name, photo);
        }
    }

    /// The avatar for the photo at `url`: kept on this device, or downloaded and made now.
    /// `None` when the photo can't be had; the contact then has no photo for now.
    async fn avatar_for(&self, url: &str) -> Option<String> {
        let avatar = match self.inner.avatars.get(url) {
            Some(avatar) => avatar,
            None => {
                let downloaded = match self.inner.messaging.download_photo(url).await {
                    Ok(downloaded) => downloaded,
                    Err(error) => {
                        log::info!("could not download a contact's photo: {error}");
                        return None;
                    }
                };
                let avatar = match photo::square_jpeg(&downloaded, AVATAR_SIDE) {
                    Ok(avatar) => avatar,
                    Err(error) => {
                        log::info!("a contact's photo can't be shown: {error}");
                        return None;
                    }
                };
                if let Err(error) = self.inner.avatars.put(url, &avatar) {
                    log::warn!("could not keep a contact's avatar: {error}");
                }
                avatar
            }
        };
        Some(photo::data_url(&avatar))
    }

    /// A contact's photo as this device has it already, without fetching anything. Starts
    /// fetching the avatar when only the photo's address is known.
    fn known_photo(&self, contact: &str) -> Option<String> {
        let url = self.inner.messaging.known_picture(contact)?;
        if let Some(avatar) = self.inner.avatars.get(&url) {
            return Some(photo::data_url(&avatar));
        }
        let core = self.clone();
        let contact = contact.to_owned();
        tokio::spawn(async move {
            let photo = core.avatar_for(&url).await;
            if photo.is_some() {
                core.update_contact(&contact, None, photo);
            }
        });
        None
    }

    fn any_relay_connected(&self) -> bool {
        self.reach()
            .relays(Instant::now())
            .iter()
            .any(|(_, status)| *status == RelayStatus::Connected)
    }

    /// Gives the contact with `public_key` their current name and photo in their chats and
    /// invitations, and emits [`CoreEvent::ContactChanged`] if that changed anything. Without a
    /// `name`, the contact keeps the one they have.
    fn update_contact(&self, public_key: &str, name: Option<String>, photo: Option<String>) {
        let mut state = self.state();
        let state = &mut *state;
        let mut changed = None;
        let contacts = state.chats.iter_mut().map(|chat| &mut chat.contact).chain(
            state
                .invitations
                .iter_mut()
                .map(|invitation| &mut invitation.from),
        );
        for contact in contacts.filter(|contact| contact.public_key == public_key) {
            let name = name.clone().unwrap_or_else(|| contact.name.clone());
            if contact.name != name || contact.photo != photo {
                contact.name = name;
                contact.photo = photo.clone();
                changed = Some(contact.clone());
            }
        }
        if let Some(contact) = changed {
            log::info!("a contact's profile changed");
            self.emit(CoreEvent::ContactChanged { contact });
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
        self.undelivered().clear();
        if let Err(error) = self.inner.avatars.clear() {
            log::warn!("could not remove the avatars: {error}");
        }
        if let Err(error) = self.inner.invites.clear() {
            log::warn!("could not remove the invitations: {error}");
        }
        if let Err(error) = self.inner.known_chats.clear() {
            log::warn!("could not remove the known chats: {error}");
        }
        self.clear_radio();
        if let Err(error) = self.inner.outbox.clear() {
            log::warn!("could not remove the waiting messages: {error}");
        }
        self.emit(CoreEvent::AccountDeleted);
        // The paired radio went with the account, but the device can still pair one (#24).
        self.radio_changed();
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
            peer_hex: invite.public_key_hex.clone(),
            form: ChatForm::Direct,
            invitation: false,
            active: true,
            relays: Vec::new(),
            disappear_after: None,
        };
        self.apply(&started, &invite.public_key_hex, name, now_ms());
        // Whether the invitation reached the contact's relays (#126).
        self.read_undelivered_soon();
        // A personal invitation's token goes back to the inviter, whose app then opens the chat
        // without a request (#95). Without it, the chat still works: as a request.
        if let Some(token) = &invite.token
            && let Err(error) = self
                .inner
                .messaging
                .send_invite_token(&account.public_key, &started.id, token)
                .await
        {
            log::warn!("could not send the invitation's token: {error}");
        }
        self.chat(&started.id)
            .ok_or_else(|| failed("start the chat", "the chat is missing after starting it"))
    }

    /// Makes a personal invitation for `name`, whom only the user sees (#95). Emits
    /// [`CoreEvent::InviteChanged`]. Its link works once and for 7 days: the person who uses it
    /// lands in a chat with the user, and the user's app opens it without a request.
    pub async fn create_invite(&self, name: &str) -> Result<SentInvite, CommandError> {
        let account = self.account()?;
        let name = name.trim();
        if name.is_empty() {
            return Err(CommandError::new("Enter a name."));
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(CommandError::new(format!(
                "A name can have up to {MAX_NAME_CHARS} characters."
            )));
        }
        let record = self
            .inner
            .invites
            .create(name, now_ms())
            .map_err(|error| failed("keep the invitation", error))?;
        log::info!("invitation created");
        let invite = invite_view(&account, &record);
        self.state().invites.push(invite.clone());
        self.emit(CoreEvent::InviteChanged {
            invite: invite.clone(),
        });
        Ok(invite)
    }

    /// Who an invite link is from: the name and photo in their profile, for the screen an
    /// invited person sees before they have an account (#95). Needs no account.
    pub async fn preview_invite(&self, invite_link: &str) -> Result<Contact, CommandError> {
        let invite =
            invite::parse(invite_link).map_err(|error| CommandError::new(error.to_string()))?;
        if self
            .state()
            .account
            .as_ref()
            .is_some_and(|account| account.public_key == invite.public_key_hex)
        {
            return Err(CommandError::new("That is your own invite link."));
        }
        let profile = self
            .inner
            .messaging
            .fetch_profile(&invite.public_key_hex, &invite.relays)
            .await
            .unwrap_or_default();
        let photo = match &profile.picture {
            Some(url) => self.avatar_for(url).await,
            None => None,
        };
        Ok(Contact {
            name: profile
                .name
                .unwrap_or_else(|| marmot_app::default_profile_pseudonym(&invite.public_key_hex)),
            public_key: invite.public_key_hex,
            photo,
            meshes: Vec::new(),
        })
    }

    /// Takes a token the other person sent in a chat (#95). If it opens one of the user's
    /// invitations, the invitation is marked as used, and the chat opens if it was still a
    /// request. A chat the snapshot doesn't have yet is looked at again when it arrives.
    fn redeem_token(&self, token: &InviteToken) {
        let (peer, pending) = {
            let state = self.state();
            if let Some(invitation) = state.invitations.iter().find(|i| i.id == token.chat_id) {
                (invitation.from.public_key.clone(), true)
            } else if let Some(chat) = state.chats.iter().find(|c| c.id == token.chat_id) {
                (chat.contact.public_key.clone(), false)
            } else {
                return;
            }
        };
        let Some(record) = self
            .inner
            .invites
            .redeem(&token.token, &peer, &token.chat_id, now_ms())
        else {
            return;
        };
        log::info!("an invitation was used");
        let Some(account) = self.state().account.clone() else {
            return;
        };
        let invite = invite_view(&account, &record);
        {
            let mut state = self.state();
            if let Some(known) = state.invites.iter_mut().find(|known| known.id == invite.id) {
                *known = invite.clone();
            }
        }
        self.emit(CoreEvent::InviteChanged { invite });
        if pending {
            let core = self.clone();
            let chat_id = token.chat_id.clone();
            tokio::spawn(async move {
                if let Err(error) = core.accept_invitation(&chat_id).await {
                    log::warn!("could not open the chat of a used invitation: {error}");
                }
            });
        }
    }

    /// Redeems the tokens already in the chat `chat_id`, which came before the snapshot had it.
    fn redeem_tokens_in(&self, chat_id: &str) {
        let Some(account) = self.state().account.clone() else {
            return;
        };
        match self
            .inner
            .messaging
            .invite_tokens(&account.public_key, chat_id)
        {
            Ok(tokens) => tokens.iter().for_each(|token| self.redeem_token(token)),
            Err(error) => log::warn!("could not read a chat's invitation tokens: {error}"),
        }
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
            form: ChatForm::Direct,
            invitation: false,
            active: true,
            relays: Vec::new(),
            // MDK's update of the chat brings its timer.
            disappear_after: None,
        };
        self.apply(
            &accepted,
            &invitation.from.public_key,
            invitation.from.name,
            invitation.received_at,
        );
        self.chat(&invitation.id)
            .ok_or_else(|| failed("accept the request", "the chat is missing after accepting"))
    }

    /// Sends `text` in the chat `chat_id` (#20). Emits [`CoreEvent::MessageAdded`], and
    /// [`CoreEvent::DeliveryChanged`] once a relay has acknowledged the message.
    ///
    /// The message is stored before this returns. While no relay can be reached, MDK keeps it
    /// and tries again, also after a restart; until then it is `waiting`.
    ///
    /// Without a reachable relay, but with the contact's radio heard on the same preset, or with
    /// the switch that forces radio on, the radio carries it too (#40, #45). It is then `radio`,
    /// and `confirmed` once the other radio confirms it.
    pub async fn send_message(&self, chat_id: &str, text: &str) -> Result<Message, CommandError> {
        let account = self.account()?;
        let text = text.trim();
        if text.is_empty() {
            return Err(CommandError::new("Write a message first."));
        }
        let Some(chat) = self.chat(chat_id) else {
            return Err(CommandError::new("This chat no longer exists."));
        };
        if let Some(why) = cannot_send(&chat) {
            return Err(CommandError::new(why));
        }
        // Radio keys are for the two people of a direct chat (#37): another chat goes by
        // internet, or waits (#125).
        let forced = self.inner.radio_store.forced() && chat.state.is_direct();
        let relays_up = self.reach().reach(chat_id, Instant::now()) == Reach::Internet;
        let radio = self.chat_radio_reach(&chat);
        match route(forced, relays_up, radio) {
            Route::Internet => self.send_by_internet(&account, chat_id, text).await,
            Route::Radio => match self.radio_ready(chat_id, text).await {
                Ok(ready) => self.send_by_radio(&account, chat_id, text, ready).await,
                // With the switch on, the user wants to know why the radio can't.
                Err(error) if forced => Err(error),
                // Chosen by the rules: too long for radio, for example. It waits for the
                // internet (#46).
                Err(error) => {
                    log::info!("a message waits for the internet: {}", error.message);
                    self.queue_message(chat_id, text)
                }
            },
            // No path: the core keeps it until one returns, and it can be cancelled (#46).
            Route::Wait => self.queue_message(chat_id, text),
        }
    }

    /// Hands `text` to MDK, which publishes it to the relays now or once one can be reached.
    async fn send_by_internet(
        &self,
        account: &Account,
        chat_id: &str,
        text: &str,
    ) -> Result<Message, CommandError> {
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
            .map(|message| self.stamp_expiry(message))
            .unwrap_or_else(|| TextMessage {
                chat_id: chat_id.to_owned(),
                id: message_id.clone(),
                outgoing: true,
                sender_hex: account.public_key.clone(),
                text: text.to_owned(),
                sent_at: now_ms() / 1000 * 1000,
                acknowledged: false,
                expires_at: self.expiry_for(chat_id, now_ms() / 1000 * 1000),
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

    /// Updates the account's own key in the chat `chat_id`. Once the relays confirm it, the
    /// chat is in a new MLS epoch. MDK does this by itself every 24 to 36 days.
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
        self.stop_radio();
        self.inner.messaging.close().await
    }

    /// Puts the account's chats and invitations into the snapshot, with their latest messages,
    /// and keeps them up to date. Chats whose contacts' names are known go in now; the rest
    /// arrive as events once their names are. Follows the relays too, for each chat's reach.
    async fn follow(&self, account_hex: &str) {
        // Chats' new epochs from before the chats are read, so that no change to a chat's state
        // is missed (#128).
        let mut epochs = self.inner.messaging.follow_epochs();
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
        // The invitations that haven't reached their contacts, before the chats, so that the
        // chats start out showing them (#126). Notices from here on reach the watcher.
        let invitation_failures = self.inner.messaging.follow_invitation_failures(account_hex);
        self.read_undelivered().await;
        let mut unnamed = Vec::new();
        for chat in chats.current() {
            let Some(contact) = self.contact_of(&chat) else {
                continue;
            };
            self.reach().set_chat_relays(&chat.id, &chat.relays);
            match self.inner.messaging.known_name(&contact) {
                Some(name) => {
                    let started = self.started_at(&chat.id);
                    self.apply(&chat, &contact, name, started);
                }
                None => unnamed.push(chat),
            }
        }
        // The relays go into the snapshot now; their watcher keeps them current.
        self.refresh_reach(Instant::now());
        // An invitation's token that came while the app was closed opens its chat now (#95).
        let pending: Vec<String> = self
            .state()
            .invitations
            .iter()
            .map(|invitation| invitation.id.clone())
            .collect();
        for chat_id in pending {
            self.redeem_tokens_in(&chat_id);
        }

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
            while let Some(item) = messages.next().await {
                let Some(core) = upgrade(&core) else { return };
                match item {
                    ChatItem::Text(message) => core.apply_message(&message),
                    ChatItem::InviteToken(token) => core.redeem_token(&token),
                    ChatItem::RadioAddress(address) => core.radio_address_received(&address),
                    ChatItem::Expired { message_id } => core.message_expired(&message_id),
                }
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
        let invitation_watcher = tokio::spawn(undelivered::watch_invitations(
            Arc::downgrade(&self.inner),
            invitation_failures,
            Arc::clone(&self.inner.invitation_wake),
        ));
        let core = Arc::downgrade(&self.inner);
        let epoch_watcher = tokio::spawn(async move {
            while let Some(change) = epochs.next().await {
                let Some(core) = upgrade(&core) else { return };
                match change {
                    EpochChange::Chat(chat_id) => core.radio_chat_changed(&chat_id),
                    EpochChange::Missed => core.refresh_radio_records().await,
                }
            }
        });
        // Disappearing messages, soon after start (#124).
        let expiry_watcher = tokio::spawn(expiry::watch_expiry(
            Arc::downgrade(&self.inner),
            Arc::clone(&self.inner.expiry),
        ));
        let mut watchers = vec![
            chat_watcher,
            message_watcher,
            contact_watcher,
            invitation_watcher,
            epoch_watcher,
            expiry_watcher,
        ];
        if let Some(relays) = relays {
            watchers.push(tokio::spawn(watch_relays(
                Arc::downgrade(&self.inner),
                relays,
                Arc::clone(&self.inner.reach_wake),
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
        let Some(contact) = self.contact_of(&chat) else {
            return;
        };
        // The relays the chat goes through may decide its reach.
        self.reach().set_chat_relays(&chat.id, &chat.relays);
        self.refresh_reach(Instant::now());
        self.radio_chat_changed(&chat.id);
        // Someone may have turned disappearing messages on or off (#124).
        self.disappearing_changed(&chat);
        if self.reflects(&chat) {
            return;
        }
        let name = self.inner.messaging.contact_name(&contact, &[]).await;
        let started = self.started_at(&chat.id);
        self.apply(&chat, &contact, name, started);
        if chat.invitation {
            self.redeem_tokens_in(&chat.id);
        }
    }

    /// Whom `chat` is with, if the core shows it: a direct chat or invitation, or a chat that
    /// was direct when the core showed it, which it keeps following by its id (#125). `None` for
    /// groups the core never showed, and for direct chats the user is no longer in.
    fn contact_of(&self, chat: &DirectChat) -> Option<String> {
        {
            let state = self.state();
            if let Some(known) = state.chats.iter().find(|known| known.id == chat.id) {
                return Some(known.contact.public_key.clone());
            }
            if let Some(invitation) = state.invitations.iter().find(|i| i.id == chat.id) {
                return Some(invitation.from.public_key.clone());
            }
        }
        if let Some(contact) = self.inner.known_chats.contact(&chat.id) {
            return Some(contact);
        }
        (chat.form == ChatForm::Direct && chat.active).then(|| chat.peer_hex.clone())
    }

    /// Whether the snapshot shows `chat` as it is already.
    fn reflects(&self, chat: &DirectChat) -> bool {
        let state = self.state();
        if let Some(known) = state.chats.iter().find(|known| known.id == chat.id) {
            return known.state == chat_state(chat);
        }
        let is_invitation = state
            .invitations
            .iter()
            .any(|invitation| invitation.id == chat.id);
        if is_invitation {
            // Still an invitation to a direct chat.
            chat.active && chat.invitation && chat.form == ChatForm::Direct
        } else {
            // A known chat that is new to the snapshot, as at start, needs adding; one that
            // ended before the core showed it needs nothing.
            !chat.active && self.inner.known_chats.contact(&chat.id).is_none()
        }
    }

    /// Records `chat` in the snapshot and emits what changed. The commands and the watchers all
    /// come through here, under the state lock, so each change is emitted once.
    ///
    /// A chat or invitation that is new to the snapshot gets its history here, under the same
    /// lock: a message stored earlier is in it, and one stored later reaches
    /// [`Self::apply_message`] after the chat is in the snapshot.
    ///
    /// `contact_hex` is whom the chat is with: for a chat that is no longer direct, the contact
    /// it had when it was (#125).
    fn apply(&self, chat: &DirectChat, contact_hex: &str, contact_name: String, started_at: u64) {
        let mut state = self.state();
        let Some(account_hex) = state
            .account
            .as_ref()
            .map(|account| account.public_key.clone())
        else {
            return;
        };
        let chat_state = chat_state(chat);
        let known = state.chats.iter().position(|known| known.id == chat.id);
        let invitation = state
            .invitations
            .iter()
            .position(|invitation| invitation.id == chat.id);
        // Only a new contact looks for a photo; the refresh keeps it current after that.
        let contact = || Contact {
            public_key: contact_hex.to_owned(),
            name: contact_name.clone(),
            photo: self.known_photo(contact_hex),
            meshes: self.contact_meshes(contact_hex),
        };

        let mut events = Vec::new();
        let mut added = None;
        if let Some(index) = known {
            // A chat that stops being direct, or ends, stays, and says so (#125).
            let known = &mut state.chats[index];
            if known.state != chat_state {
                log::info!("a chat is now {chat_state:?}");
                known.state = chat_state;
                // In a chat that is no longer direct, messages say who sent them.
                known.messages = self.history(&account_hex, &chat.id, chat_state);
                events.push(CoreEvent::ChatChanged {
                    chat: known.clone(),
                });
            }
        } else if let Some(index) = invitation {
            if !chat.active || chat.form != ChatForm::Direct {
                // Declined elsewhere, withdrawn, or no longer a direct chat.
                state.invitations.remove(index);
                events.push(CoreEvent::InvitationRemoved {
                    invitation_id: chat.id.clone(),
                });
            } else if !chat.invitation {
                // Accepted.
                let accepted = state.invitations.remove(index);
                events.push(CoreEvent::InvitationRemoved {
                    invitation_id: chat.id.clone(),
                });
                added = Some(accepted.from);
            }
        } else if chat.invitation {
            if chat.active && chat.form == ChatForm::Direct {
                let invitation = Invitation {
                    id: chat.id.clone(),
                    from: contact(),
                    received_at: started_at,
                    messages: self.history(&account_hex, &chat.id, ChatState::Direct),
                };
                state.invitations.push(invitation.clone());
                events.push(CoreEvent::InvitationReceived { invitation });
            }
        } else {
            // A new direct chat, or, as at start, one the core showed before it changed.
            added = Some(contact());
        }
        if let Some(contact) = added {
            if chat_state.is_direct() {
                self.inner
                    .known_chats
                    .remember(&chat.id, &contact.public_key);
            }
            let added = Chat {
                id: chat.id.clone(),
                contact,
                state: chat_state,
                messages: self.history(&account_hex, &chat.id, chat_state),
                reach: self.reach().reach(&chat.id, Instant::now()),
                // The radio's part follows right after, from `radio_chat_added`.
                radio_issue: None,
                undelivered: undelivered::view_for(&self.undelivered(), &chat.id),
                disappear_after: chat.disappear_after,
            };
            state.chats.push(added.clone());
            events.push(CoreEvent::ChatAdded { chat: added });
        }
        let added = events
            .iter()
            .any(|event| matches!(event, CoreEvent::ChatAdded { .. }));
        let changed = events
            .iter()
            .any(|event| matches!(event, CoreEvent::ChatChanged { .. }));
        for event in events {
            self.emit(event);
        }
        drop(state);
        if added && chat_state.is_direct() {
            self.radio_chat_added(&chat.id);
        }
        if changed {
            // Only a direct chat goes by radio.
            self.refresh_reach(Instant::now());
        }
    }

    /// Records a message that was added or changed, and emits what changed: a new message, or
    /// an outgoing one that a relay has now acknowledged. A message for a chat the snapshot
    /// doesn't have yet is in that chat's history when it arrives.
    fn apply_message(&self, message: &TextMessage) {
        // The user's own message may come before MDK has stamped its expiry (#124).
        let stamped;
        let message = if message.outgoing && message.expires_at.is_none() {
            stamped = self.stamp_expiry(message.clone());
            &stamped
        } else {
            message
        };
        let mut view = message_view(
            message,
            self.inner.radio_store.message(&message.id).as_ref(),
        );
        let waits = message.outgoing && self.radio_sending_in(&message.chat_id);
        // Gone already (#124); one that expires later wakes the watcher, which takes it then.
        if let Some(at) = view.expires_at {
            if at <= now_ms() {
                return;
            }
            self.inner.expiry.look_again();
        }
        let mut guard = self.state();
        let state = &mut *guard;
        let messages = if let Some(chat) = state.chats.iter_mut().find(|c| c.id == message.chat_id)
        {
            view.sender = self.sender_in(chat.state, message);
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
            // A radio send in the chat adds its own message when it is done, as radio (#40).
            None if waits => return,
            None => {
                messages.push(view.clone());
                CoreEvent::MessageAdded {
                    chat_id: message.chat_id.clone(),
                    message: view,
                }
            }
            // Delivery only moves forward: waiting, timed out, sent, confirmed.
            Some(known)
                if delivery_rank(view.delivery) > delivery_rank(known.delivery)
                    && let Some(delivery) = view.delivery =>
            {
                known.delivery = view.delivery;
                CoreEvent::DeliveryChanged {
                    chat_id: message.chat_id.clone(),
                    message_id: view.id,
                    delivery,
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
            // Invitations that couldn't be delivered go again now too (#126).
            self.inner.invitation_wake.notify_one();
        }
        self.refresh_reach(now)
    }

    /// Sets each chat's reach and each relay's status as they are at `now`, and emits
    /// [`CoreEvent::ReachChanged`] and [`CoreEvent::RelaysChanged`] for what changed. Returns
    /// when a closed connection next makes its relay count as gone.
    fn refresh_reach(&self, now: Instant) -> Option<Instant> {
        // Without the internet, a chat may be reachable by radio (#44), while its airtime lasts
        // (#51).
        let own_radio = self.own_radio_record();
        let (airtime_left, airtime_frees) = self.airtime_left();
        let now_epoch_ms = now_ms();
        let mut state = self.state();
        let reachability = self.reach();
        let heard = self.heard();
        let relays = relay_views(reachability.relays(now), PROJECT_RELAYS);
        if state.account.is_some() && state.relays != relays {
            state.relays = relays.clone();
            self.emit(CoreEvent::RelaysChanged { relays });
        }
        for chat in &mut state.chats {
            let (reach, radio_issue) = match reachability.reach(&chat.id, now) {
                Reach::Internet => (Reach::Internet, None),
                // Radio keys are for the two people of a direct chat (#125).
                _ if !chat.state.is_direct() => (Reach::Unreachable, None),
                _ => {
                    let contact = self
                        .inner
                        .radio_store
                        .contact_radio(&chat.contact.public_key);
                    match radio_reach(
                        own_radio.as_ref(),
                        contact.as_ref(),
                        &heard,
                        airtime_left,
                        now,
                    ) {
                        Ok(()) => (Reach::Radio, None),
                        Err(issue) => (Reach::Unreachable, Some(issue)),
                    }
                }
            };
            if chat.reach != reach || chat.radio_issue != radio_issue {
                chat.reach = reach;
                chat.radio_issue = radio_issue;
                log::info!("a chat's reach is now {reach:?}, {radio_issue:?}");
                self.emit(CoreEvent::ReachChanged {
                    chat_id: chat.id.clone(),
                    reach,
                    radio_issue,
                });
            }
        }
        // Once airtime comes free, radio may carry messages again.
        let airtime_back = airtime_frees
            .filter(|_| !airtime_left)
            .map(|at| now + Duration::from_millis(at.saturating_sub(now_epoch_ms)));
        let next = [
            reachability.next_change(now),
            heard.next_change(now),
            airtime_back,
        ]
        .into_iter()
        .flatten()
        .min();
        // Waiting messages go where a path is (#46).
        let with_path: Vec<String> = state
            .chats
            .iter()
            .filter(|chat| chat.reach != Reach::Unreachable)
            .map(|chat| chat.id.clone())
            .collect();
        drop(heard);
        drop(reachability);
        drop(state);
        for chat_id in with_path {
            self.send_waiting_soon(&chat_id);
        }
        next
    }

    /// Looks at each chat's reach again now, and wakes the relays' watcher to look again when it
    /// changes next.
    fn reach_changed(&self) {
        self.refresh_reach(Instant::now());
        self.inner.reach_wake.notify_one();
    }

    /// The latest messages of a chat or invitation, with those that came by radio and whose
    /// relay copies haven't arrived yet (#40). In a chat that is no longer direct, messages
    /// say who sent them (#125).
    fn history(&self, account_hex: &str, chat_id: &str, chat_state: ChatState) -> Vec<Message> {
        let store = &self.inner.radio_store;
        let mut messages: Vec<Message> =
            match self.inner.messaging.chat_messages(account_hex, chat_id) {
                Ok(messages) => messages
                    .iter()
                    .map(|message| Message {
                        sender: self.sender_in(chat_state, message),
                        ..message_view(message, store.message(&message.id).as_ref())
                    })
                    .collect(),
                Err(error) => {
                    log::warn!("could not read a chat's messages: {error}");
                    Vec::new()
                }
            };
        let kept = store.messages_in(chat_id);
        let mut more: Vec<Message> = kept
            .iter()
            .filter(|radio| {
                !radio.expired && !messages.iter().any(|message| message.id == radio.id)
            })
            .map(radio_view)
            .collect();
        // Messages that wait for a path (#46).
        more.extend(self.queued_messages(chat_id));
        if !more.is_empty() {
            messages.extend(more);
            // Times are whole seconds. Within one, what this phone kept first comes first.
            let kept_at = |id: &str| {
                kept.iter()
                    .find(|radio| radio.id == id)
                    .map_or(0, |radio| radio.kept_at)
            };
            messages.sort_by_key(|message| (message.sent_at, kept_at(&message.id)));
        }
        // Those whose time is up are gone, whether or not MDK has deleted them yet (#124).
        let now = now_ms();
        messages.retain(|message| message.expires_at.is_none_or(|at| at > now));
        messages
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

    /// The direct chat with `public_key`. A chat with them that is no longer direct doesn't
    /// count: adding them again starts a new one (#125).
    fn chat_with(&self, public_key: &str) -> Option<Chat> {
        self.state()
            .chats
            .iter()
            .find(|chat| chat.contact.public_key == public_key && chat.state.is_direct())
            .cloned()
    }

    /// Who sent `message`, for a chat in `chat_state` (#125): see [`message_sender`].
    fn sender_in(&self, chat_state: ChatState, message: &TextMessage) -> Option<Sender> {
        message_sender(chat_state, message, |hex| {
            self.inner.messaging.known_name(hex)
        })
    }

    /// Whether `chat` can be reached by radio: only a direct chat can (#125).
    pub(crate) fn chat_radio_reach(&self, chat: &Chat) -> Result<(), RadioIssue> {
        if chat.state.is_direct() {
            self.contact_radio_reach(&chat.contact.public_key)
        } else {
            Err(RadioIssue::NoContactRadio)
        }
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
async fn watch_relays(core: Weak<Inner>, relays: RelayLinks, wake: Arc<tokio::sync::Notify>) {
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
            // A radio was heard, or the radio changed: when the reach changes next may be sooner.
            () = wake.notified() => None,
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

/// An invitation as an interface sees it, with the link to share.
fn invite_view(account: &Account, record: &InviteRecord) -> SentInvite {
    SentInvite {
        id: record.token.clone(),
        name: record.name.clone(),
        link: invite::personal_link(&account.invite_link, &record.token),
        created_at: record.created_at,
        expires_at: record.expires_at,
        joined_chat_id: record.joined.as_ref().map(|joined| joined.chat_id.clone()),
    }
}

fn invite_views(account: &Account, records: &[InviteRecord]) -> Vec<SentInvite> {
    records
        .iter()
        .map(|record| invite_view(account, record))
        .collect()
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
    avatars: &Avatars,
    public_key_hex: String,
    name: String,
) -> Result<Account, MessagingError> {
    let invite_link = invite::invite_link(&public_key_hex, messaging.relays())?;
    let share_link = invite::web_link(&invite_link);
    // The invite link above has already read the same key.
    let npub = invite::npub(&public_key_hex).expect("the invite link has read this key");
    let photo = messaging
        .known_picture(&public_key_hex)
        .and_then(|url| avatars.get(&url))
        .map(|avatar| photo::data_url(&avatar));
    Ok(Account {
        name,
        about: messaging.own_about(&public_key_hex),
        photo,
        invite_link,
        share_link,
        npub,
        public_key: public_key_hex,
    })
}

/// What `chat` is now, as an interface shows it (#125).
fn chat_state(chat: &DirectChat) -> ChatState {
    if !chat.active {
        return ChatState::Removed;
    }
    match chat.form {
        ChatForm::Direct => ChatState::Direct,
        ChatForm::Alone => ChatState::ContactLeft,
        ChatForm::Group => ChatState::Group,
    }
}

/// Who sent `message`, in a chat in `chat_state` (#125). Only incoming messages in a `group`
/// chat say so: there anyone in the chat can have sent them. In a direct chat, and one the
/// contact left, the contact sent them all. `known_name` gives a name from a profile; without
/// one the sender goes by MDK's pseudonym for them, as in White Noise.
fn message_sender(
    chat_state: ChatState,
    message: &TextMessage,
    known_name: impl FnOnce(&str) -> Option<String>,
) -> Option<Sender> {
    if chat_state != ChatState::Group || message.outgoing || message.sender_hex.is_empty() {
        return None;
    }
    let hex = &message.sender_hex;
    Some(Sender {
        public_key: hex.clone(),
        name: known_name(hex).unwrap_or_else(|| marmot_app::default_profile_pseudonym(hex)),
    })
}

/// Why nothing can be sent in `chat`, if it has ended (#125).
fn cannot_send(chat: &Chat) -> Option<String> {
    match chat.state {
        ChatState::ContactLeft => Some(format!(
            "{} left this chat. Messages can't be sent here any more.",
            chat.contact.name
        )),
        ChatState::Removed => Some("You are no longer in this chat.".to_owned()),
        ChatState::Direct | ChatState::Group => None,
    }
}

/// A message as an interface sees it. `radio` is the message's record in the radio store, if
/// it travelled by radio (#40).
fn message_view(message: &TextMessage, radio: Option<&RadioMessage>) -> Message {
    let delivery = if radio.is_some_and(|radio| radio.confirmed) {
        DeliveryState::Confirmed
    } else if message.acknowledged {
        DeliveryState::Sent
    } else if radio.is_some_and(|radio| radio.timed_out) {
        DeliveryState::TimedOut
    } else {
        DeliveryState::Waiting
    };
    Message {
        id: message.id.clone(),
        direction: if message.outgoing {
            Direction::Out
        } else {
            Direction::In
        },
        text: message.text.clone(),
        sent_at: message.sent_at,
        transport: if radio.is_some() {
            Transport::Radio
        } else {
            Transport::Internet
        },
        delivery: message.outgoing.then_some(delivery),
        cancellable: false,
        sender: None,
        expires_at: message
            .expires_at
            .or_else(|| radio.and_then(|radio| radio.expires_at)),
    }
}

/// A message that came by radio, before its relay copy has.
fn radio_view(radio: &RadioMessage) -> Message {
    Message {
        id: radio.id.clone(),
        direction: if radio.outgoing {
            Direction::Out
        } else {
            Direction::In
        },
        text: radio.text.clone(),
        sent_at: radio.sent_at,
        transport: Transport::Radio,
        delivery: radio.outgoing.then_some(if radio.confirmed {
            DeliveryState::Confirmed
        } else if radio.timed_out {
            DeliveryState::TimedOut
        } else {
            DeliveryState::Waiting
        }),
        cancellable: false,
        sender: None,
        expires_at: radio.expires_at,
    }
}

/// How far a message's delivery has come. A radio message that timed out has come further
/// than one still waiting for confirmation, and less far than one a relay acknowledged (#41).
fn delivery_rank(delivery: Option<DeliveryState>) -> u8 {
    match delivery {
        None => 0,
        Some(DeliveryState::Waiting) => 1,
        Some(DeliveryState::TimedOut) => 2,
        Some(DeliveryState::Sent) => 3,
        Some(DeliveryState::Confirmed) => 4,
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

    fn text_from(sender_hex: &str, outgoing: bool) -> TextMessage {
        TextMessage {
            chat_id: "chat".to_owned(),
            id: "message".to_owned(),
            outgoing,
            sender_hex: sender_hex.to_owned(),
            text: "Hello".to_owned(),
            sent_at: 0,
            acknowledged: outgoing,
            expires_at: None,
        }
    }

    #[test]
    fn issue_125_incoming_messages_in_a_group_say_who_sent_them() {
        let cy = "c".repeat(64);
        let named = |hex: &str| (hex == cy).then(|| "Cy Berg".to_owned());

        // A known name.
        let sender = message_sender(ChatState::Group, &text_from(&cy, false), named).unwrap();
        assert_eq!(sender.public_key, cy);
        assert_eq!(sender.name, "Cy Berg");

        // Without a profile, the pseudonym MDK and White Noise give them.
        let dee = "d".repeat(64);
        let sender = message_sender(ChatState::Group, &text_from(&dee, false), named).unwrap();
        assert_eq!(sender.public_key, dee);
        assert_eq!(sender.name, marmot_app::default_profile_pseudonym(&dee));
        assert!(!sender.name.is_empty());
    }

    #[test]
    fn issue_125_only_incoming_messages_in_a_group_carry_a_sender() {
        let cy = "c".repeat(64);
        let named = |_: &str| Some("Cy Berg".to_owned());
        // Our own messages, in any chat.
        for state in [
            ChatState::Direct,
            ChatState::ContactLeft,
            ChatState::Removed,
            ChatState::Group,
        ] {
            assert_eq!(message_sender(state, &text_from(&cy, true), named), None);
        }
        // The contact's, in a chat that is or was direct, or one the user left.
        for state in [
            ChatState::Direct,
            ChatState::ContactLeft,
            ChatState::Removed,
        ] {
            assert_eq!(message_sender(state, &text_from(&cy, false), named), None);
        }
        // A message MDK gave no sender for.
        assert_eq!(
            message_sender(ChatState::Group, &text_from("", false), named),
            None
        );
    }
}
