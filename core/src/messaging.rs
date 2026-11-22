//! Internet messaging: Marmot over Nostr relays, through MDK's multi-account app runtime.
//!
//! This is the link to MDK (#59): open the runtime, create, list and delete accounts, and wait
//! until an account's relay lists, profile and KeyPackage are on the relays. Direct chats (#19):
//! start one with another account, follow the account's chats and invitations, and accept an
//! invitation. Text (#20): send it, read a chat's history and follow new messages and their
//! delivery. Relays (#43): follow whether the account's connections to its relays are open.
//! [`crate::Core`] builds the interface's commands and events on it (#16).
//!
//! Account keys stay inside MDK (#17). It keeps them in the platform's key store and signs with
//! them itself, and an external signer can take their place. Nothing here reads them. Details:
//! `docs/mdk.md`, "The account key".

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cgka_traits::{GroupId, TransportEndpoint};
use marmot_account::AccountHome;
use marmot_app::{
    AccountSetupReadiness, AccountSetupRequest, AppError, AppGroupLifecycleState, AppGroupMlsState,
    AppGroupRecord, MarmotApp, MarmotAppConfig, MarmotAppRuntime, RelayLiveness,
    RelayLivenessStatus, RuntimeChatsSubscription, RuntimeRelayLivenessSubscription,
    RuntimeTimelineMessageUpdate, RuntimeTimelineMessagesSubscription, SelfMembership,
    TimelineMessageChange, TimelineMessageQuery, TimelineMessageRecord, TimelinePagination,
    UserProfileMetadata,
};
use nostr::types::RelayUrl;
use zeroize::Zeroizing;

/// How often [`Messaging::wait_until_published`] checks the account's setup state.
const READINESS_POLL: Duration = Duration::from_millis(50);
/// How long [`Messaging::contact_name`] waits for a profile from the relays.
const PROFILE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long [`Messaging::accept_direct_chat`] retries while the account is busy, as `wn` does.
const ACCEPT_RETRY_FOR: Duration = Duration::from_secs(30);
const ACCEPT_RETRY_POLL: Duration = Duration::from_millis(50);
/// How many of a chat's latest messages [`Messaging::chat_messages`] reads: MDK's largest page.
const HISTORY_LIMIT: usize = 200;
/// MDK's name for `MLS-Exporter("marmot", "group-event", 32)`, one of the three exporter secrets
/// it computes for every epoch and lets the app read.
const GROUP_EVENT_EXPORTER: &str = "marmot/group-event";
const EXPORTER_BYTES: usize = 32;
/// The surface that MDK's audit log records for a key export: a fixed label, never a person or a
/// key.
const EXPORT_CALLER: &str = "peoples_chat_core::Core::export_account_key";
/// How often [`Messaging::chat_secret`] tries while the chat is changing, and how long it waits
/// in between.
const SECRET_ATTEMPTS: usize = 5;
const SECRET_RETRY: Duration = Duration::from_millis(100);
/// MDK's kind for chat messages (NIP-C7).
const CHAT_KIND: u64 = 9;
/// The mark MDK puts on a message of ours that no relay would take.
const LOCAL_PUBLISH_FAILED: &str = "local_publish_failed";
/// Makes each send's client token unique within this process.
static SENDS: AtomicU64 = AtomicU64::new(0);

/// Relays for new accounts unless the app is built with others (#18). New accounts publish their
/// relay lists, profile and KeyPackage here.
///
/// These are public, general-purpose relays. Who runs the project's own relays is decided later
/// (#11). nos.lol and relay.primal.net are also among the relays White Noise searches for relay
/// lists, so White Noise users can find accounts made here.
pub const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
];

/// Relays the project runs, which the interface shows as run by peoples.chat (#88). None yet:
/// who runs the project's relays is decided later (#11).
pub const PROJECT_RELAYS: &[&str] = &[];

/// Where the core keeps its messaging data and which relays it starts with.
#[derive(Clone, Debug)]
pub struct MessagingOptions {
    /// Folder for MDK's encrypted databases. One installation of the app uses one folder, and
    /// only one runtime at a time may open it.
    pub data_dir: PathBuf,
    /// Relays for new accounts: their relay lists, profile and KeyPackage are published here.
    pub relays: Vec<String>,
    /// Keychain service under which account keys are stored, one entry per account.
    pub keychain_service: String,
    /// Allows `ws://` relays on the loopback interface, for tests and local development.
    pub allow_loopback_relays: bool,
}

/// An account created on this device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAccount {
    /// The account's public key, in hex. MDK also uses it as the account's label.
    pub public_key_hex: String,
}

/// An account on this device that can sign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExistingAccount {
    /// The account's public key, in hex.
    pub public_key_hex: String,
    /// The profile name, when MDK knows it.
    pub name: Option<String>,
}

/// A direct chat as MDK holds it: an unnamed group of exactly two accounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectChat {
    /// The MLS group id, in hex. It stays the same for the chat's life.
    pub id: String,
    /// The other person's public key, in hex.
    pub peer_hex: String,
    /// The other person invited the account, and it has not accepted yet.
    pub invitation: bool,
    /// The account is still in the chat: the chat is not archived, left or removed.
    pub active: bool,
    /// The relays the chat's messages go through, as [`canonical_relay`] spells them. Empty when
    /// the core made this value itself rather than reading it from MDK.
    pub relays: Vec<String>,
}

/// The account's direct chats as they were when following began, then each one that changes.
pub struct DirectChats {
    account_hex: String,
    subscription: RuntimeChatsSubscription,
}

impl DirectChats {
    /// The direct chats when following began, invitations included.
    pub fn current(&self) -> Vec<DirectChat> {
        self.subscription
            .snapshot
            .iter()
            .filter_map(|record| direct_chat(&self.account_hex, record))
            .collect()
    }

    /// The next direct chat that was added or changed. `None` once messaging has stopped.
    pub async fn next(&mut self) -> Option<DirectChat> {
        loop {
            let record = self.subscription.recv().await?;
            if let Some(chat) = direct_chat(&self.account_hex, &record) {
                return Some(chat);
            }
        }
    }
}

/// A text message in a direct chat, as MDK holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextMessage {
    /// The id of the chat it belongs to.
    pub chat_id: String,
    /// MDK's id for the message: the id of its signed inner event. Both phones know the message by
    /// it, from sending through every restart.
    pub id: String,
    /// The account sent it, rather than received it.
    pub outgoing: bool,
    pub text: String,
    /// When the sender sent it, by the sender's clock, in milliseconds since the Unix epoch. MDK
    /// keeps whole seconds.
    pub sent_at: u64,
    /// Outgoing only: a relay has acknowledged it.
    pub acknowledged: bool,
}

/// The text message in `record`, unless it is something else: a system row, media, a sticker,
/// a deleted message, or one withdrawn when the chat's state was settled.
fn text_message(record: &TimelineMessageRecord) -> Option<TextMessage> {
    let outgoing = match record.direction.as_str() {
        "sent" => true,
        "received" => false,
        _ => return None,
    };
    let sticker = record
        .tags
        .iter()
        .any(|tag| tag.first().is_some_and(|name| name == "sticker"));
    // A message no relay would take stays: MDK keeps it, and it shows as not sent yet.
    let withdrawn = record
        .invalidation_status
        .as_deref()
        .is_some_and(|status| status != LOCAL_PUBLISH_FAILED);
    if record.kind != CHAT_KIND
        || record.deleted
        || record.media.is_some()
        || record.agent_text_stream.is_some()
        || sticker
        || withdrawn
    {
        return None;
    }
    Some(TextMessage {
        chat_id: record.group_id_hex.clone(),
        id: record.message_id_hex.clone(),
        outgoing,
        text: record.plaintext.clone(),
        sent_at: record.timeline_at.saturating_mul(1000),
        acknowledged: outgoing
            && (record.source_message_id_hex.is_some() || record.recipient_confirmed),
    })
}

/// The account's text messages in every chat as they are added or change, for example when a
/// relay acknowledges one of ours.
pub struct TextMessages {
    subscription: RuntimeTimelineMessagesSubscription,
    ready: VecDeque<TextMessage>,
}

impl TextMessages {
    /// The next message that was added or changed. `None` once messaging has stopped.
    pub async fn next(&mut self) -> Option<TextMessage> {
        loop {
            if let Some(message) = self.ready.pop_front() {
                return Some(message);
            }
            match self.subscription.recv().await? {
                // MDK fell behind and sends its latest messages again.
                RuntimeTimelineMessageUpdate::Page { page } => {
                    self.ready
                        .extend(page.messages.iter().filter_map(text_message));
                }
                RuntimeTimelineMessageUpdate::Projection(projection) => {
                    for change in &projection.update.timeline_changes {
                        if let TimelineMessageChange::Upsert { message, .. } = change {
                            self.ready.extend(text_message(message));
                        }
                    }
                }
            }
        }
    }
}

/// The direct chat in `record`, unless it is a group or the account's own device group.
fn direct_chat(account_hex: &str, record: &AppGroupRecord) -> Option<DirectChat> {
    let members = record.direct_member_ids_hex.as_ref()?;
    if members.len() != 2 || !members.iter().any(|member| member == account_hex) {
        return None;
    }
    let peer_hex = members.iter().find(|member| *member != account_hex)?;
    Some(DirectChat {
        id: record.group_id_hex.clone(),
        peer_hex: peer_hex.clone(),
        invitation: record.pending_confirmation,
        active: !record.archived && matches!(record.self_membership, SelfMembership::Member),
        relays: record
            .nostr_routing
            .relays
            .iter()
            .map(|relay| canonical_relay(relay))
            .collect(),
    })
}

/// One of the account's relays, and whether the account's connection to it is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayLink {
    /// As [`canonical_relay`] spells it.
    pub url: String,
    pub connected: bool,
}

fn relay_links(rows: &[RelayLiveness]) -> Vec<RelayLink> {
    rows.iter()
        .map(|row| RelayLink {
            url: canonical_relay(&row.url),
            // Connecting and not yet tried count as not connected: what matters is whether
            // messages can go out now.
            connected: row.status == RelayLivenessStatus::Connected,
        })
        .collect()
}

/// The account's relays and their connections as they were when following began, then each
/// change. MDK reports a connection as it opens and closes, and tries a closed one again every
/// 5 seconds. A connection that stops answering without closing counts as closed once MDK's
/// keep-alive ping goes unanswered.
pub struct RelayLinks {
    subscription: RuntimeRelayLivenessSubscription,
}

impl RelayLinks {
    /// The relays when following began.
    pub fn current(&self) -> Vec<RelayLink> {
        relay_links(&self.subscription.snapshot)
    }

    /// All the relays again, after one of them changed. `None` once messaging has stopped.
    pub async fn next(&self) -> Option<Vec<RelayLink>> {
        match self.subscription.recv().await {
            Ok(rows) => rows.map(|rows| relay_links(&rows)),
            Err(error) => {
                log::warn!("stopped following the relays: {error}");
                None
            }
        }
    }
}

/// One spelling per relay, so that a chat's relays and the account's can be compared: the
/// parsed URL without a bare trailing slash, as MDK spells them. Text that doesn't parse stays as
/// it is, trimmed.
pub fn canonical_relay(relay: &str) -> String {
    let relay = relay.trim();
    let mut spelled =
        RelayUrl::parse(relay).map_or_else(|_| relay.to_owned(), |url| url.to_string());
    let authority = spelled.find("://").map_or(0, |index| index + 3);
    if spelled[authority..]
        .find('/')
        .is_some_and(|slash| authority + slash + 1 == spelled.len())
    {
        spelled.pop();
    }
    spelled
}

#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    #[error(transparent)]
    Mdk(#[from] AppError),
    #[error("the account was not published to its relays within {0:?}")]
    PublicationTimeout(Duration),
    #[error("setting up the account stopped and needs recovery")]
    RecoveryRequired,
    #[error("{0:?} is not a chat id")]
    InvalidChatId(String),
    #[error("the chat is changing: a change to it waits for the relays")]
    EpochUnsettled,
}

/// A secret that both members of a chat hold in one MLS epoch, and that epoch (#37).
pub struct ChatSecret {
    pub epoch: u64,
    /// `MLS-Exporter("marmot", "group-event", 32)`. MDK derives the key for the chat's events on
    /// the relays from it, so it never leaves the core.
    pub secret: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for ChatSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatSecret")
            .field("epoch", &self.epoch)
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl MessagingError {
    /// The other person's relay lists or KeyPackage could not be found, so they can't be invited.
    pub fn is_contact_unreachable(&self) -> bool {
        matches!(
            self,
            Self::Mdk(
                AppError::MissingKeyPackage { .. }
                    | AppError::MissingMemberInboxRoute(_)
                    | AppError::MissingRelayLists(_)
                    | AppError::DirectoryLookupIncomplete(_)
            )
        )
    }
}

/// The messaging side of the core: MDK's runtime over the stores in one data folder.
pub struct Messaging {
    app: MarmotApp,
    runtime: MarmotAppRuntime,
    relays: Vec<String>,
}

impl Messaging {
    /// Opens the stores in the data folder and starts the runtime. Account keys live in the
    /// platform's key store. Must run inside a Tokio runtime, which MDK's relay tasks use.
    pub async fn open(options: MessagingOptions) -> Result<Self, MessagingError> {
        let home = AccountHome::open_with_keychain(&options.data_dir, options.keychain_service)
            .map_err(AppError::from)?;
        let config = MarmotAppConfig::default()
            .with_allow_loopback_relay_endpoints(options.allow_loopback_relays);
        let app = MarmotApp::try_with_relays_and_account_home_and_config(
            &options.data_dir,
            options.relays.clone(),
            home,
            config,
        )?;
        let runtime = app.runtime();
        runtime.start().await?;
        Ok(Self {
            app,
            runtime,
            relays: options.relays,
        })
    }

    /// The relays new accounts publish to. Invite links name them as hints.
    pub fn relays(&self) -> &[String] {
        &self.relays
    }

    /// Creates an account with a new key and `name` as its profile name.
    ///
    /// Returns once the account exists on this device. Publishing its relay lists, profile and
    /// KeyPackage continues in the background; [`Self::wait_until_published`] waits for it.
    pub async fn create_account(&self, name: &str) -> Result<NewAccount, MessagingError> {
        let relays = endpoints(&self.relays);
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
        let created = self
            .runtime
            .create_identity_local_ready_with_initial_profile(request, profile)
            .await?;
        Ok(NewAccount {
            public_key_hex: created.account.account_id_hex,
        })
    }

    /// The accounts on this device that can sign, with their profile names when known.
    pub fn accounts(&self) -> Result<Vec<ExistingAccount>, MessagingError> {
        let accounts = self.runtime.accounts().managed_accounts()?;
        Ok(accounts
            .into_iter()
            .filter(|account| !account.signed_out && !account.unlinked)
            .map(|account| ExistingAccount {
                name: self
                    .runtime
                    .display_name_for_account_id(&account.account_id_hex),
                public_key_hex: account.account_id_hex,
            })
            .collect())
    }

    /// Signs the account out and removes it from this device: its databases and its key in the
    /// keychain. Telling relays and groups is best effort and does not block the local wipe.
    pub async fn delete_account(&self, public_key_hex: &str) -> Result<(), MessagingError> {
        self.runtime.sign_out_and_wipe(public_key_hex).await?;
        Ok(())
    }

    /// Waits until the relays have acknowledged the account's relay lists, profile and
    /// KeyPackage, so that others can invite it.
    pub async fn wait_until_published(
        &self,
        account: &NewAccount,
        timeout: Duration,
    ) -> Result<(), MessagingError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .runtime
                .account_setup_readiness(&account.public_key_hex)?
            {
                AccountSetupReadiness::NetworkReady => return Ok(()),
                AccountSetupReadiness::RecoveryRequired => {
                    return Err(MessagingError::RecoveryRequired);
                }
                _ if Instant::now() >= deadline => {
                    return Err(MessagingError::PublicationTimeout(timeout));
                }
                _ => tokio::time::sleep(READINESS_POLL).await,
            }
        }
    }

    /// Starts a direct chat between the account and `peer_hex`, or finds the one they already
    /// have. Returns the chat's id.
    ///
    /// MDK looks up the other person's relay lists and KeyPackage, on our relays and on the
    /// relays their list names, and then sends them an encrypted invitation in the background.
    /// `relay_hints`, from their invite link, are searched first.
    pub async fn start_direct_chat(
        &self,
        account_hex: &str,
        peer_hex: &str,
        relay_hints: &[String],
    ) -> Result<String, MessagingError> {
        if !relay_hints.is_empty() {
            // MDK keeps the relay lists it finds here, and the lookup below uses them too. If the
            // hints lead nowhere, that lookup still searches our relays.
            if let Err(error) = self
                .app
                .directory()
                .fetch_account_relay_list_status_for_account_id(peer_hex, endpoints(relay_hints))
                .await
            {
                log::info!("the invite link's relay hints gave no relay lists: {error}");
            }
        }
        let group_id = self
            .runtime
            .start_direct_conversation(account_hex, peer_hex)
            .await?;
        Ok(group_id.to_string())
    }

    /// Accepts the invitation to the direct chat `chat_id`. Nothing goes back to the person who
    /// sent it: they are in the chat already.
    pub async fn accept_direct_chat(
        &self,
        account_hex: &str,
        chat_id: &str,
    ) -> Result<(), MessagingError> {
        let group_id = group_id(chat_id)?;
        let deadline = Instant::now() + ACCEPT_RETRY_FOR;
        loop {
            match self
                .runtime
                .accept_group_invite(account_hex, &group_id)
                .await
            {
                Ok(_) => return Ok(()),
                // The account is still catching up after a start, or has not stored the
                // invitation yet.
                Err(AppError::AccountWorkerBusy | AppError::UnknownGroup(_))
                    if Instant::now() < deadline =>
                {
                    tokio::time::sleep(ACCEPT_RETRY_POLL).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Follows the account's direct chats, invitations included.
    pub async fn follow_direct_chats(
        &self,
        account_hex: &str,
    ) -> Result<DirectChats, MessagingError> {
        let subscription = self.runtime.subscribe_chats(account_hex, false).await?;
        Ok(DirectChats {
            account_hex: account_hex.to_owned(),
            subscription,
        })
    }

    /// The name another account goes by, if this device has seen its profile.
    pub fn known_name(&self, account_hex: &str) -> Option<String> {
        self.runtime
            .display_name_for_account_id(account_hex)
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
    }

    /// The name another account goes by. Fetches its profile from `relay_hints` and our relays
    /// when this device hasn't seen it; MDK keeps what it finds. Without a profile, the name is
    /// the pseudonym MDK derives from the key, which White Noise shows too.
    pub async fn contact_name(&self, account_hex: &str, relay_hints: &[String]) -> String {
        if let Some(name) = self.known_name(account_hex) {
            return name;
        }
        let relays: Vec<String> = relay_hints
            .iter()
            .chain(&self.relays)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let fetched = tokio::time::timeout(
            PROFILE_TIMEOUT,
            self.runtime
                .fetch_current_user_profile_for_account_id(account_hex, endpoints(&relays)),
        )
        .await;
        match fetched {
            Ok(Ok(Some(profile))) => {
                if let Some(name) = profile_name(&profile) {
                    return name;
                }
            }
            Ok(Ok(None)) => log::info!("the contact has no profile on the relays"),
            Ok(Err(error)) => log::info!("could not fetch the contact's profile: {error}"),
            Err(_) => log::info!("fetching the contact's profile timed out"),
        }
        marmot_app::default_profile_pseudonym(account_hex)
    }

    /// Sends `text` in the chat `chat_id` and returns the message's id.
    ///
    /// MDK stores the message before this returns, and publishes it in the background. While no
    /// relay can be reached it keeps the message and tries again; [`Self::follow_messages`] tells
    /// when a relay has acknowledged it.
    pub async fn send_text(
        &self,
        account_hex: &str,
        chat_id: &str,
        text: &str,
    ) -> Result<String, MessagingError> {
        let group_id = group_id(chat_id)?;
        let mut collided = false;
        loop {
            match self
                .runtime
                .submit_text(account_hex, &group_id, text.to_owned(), client_token())
                .await
            {
                Ok(accepted) => return Ok(accepted.message_id_hex),
                // A message's id covers its text and the second it was sent, so the same text
                // twice within a second would be one message. Send it in the next second.
                Err(AppError::InvalidAppMessagePayload(reason))
                    if !collided && reason.starts_with("message identity collision") =>
                {
                    collided = true;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// The latest messages of the chat `chat_id`, up to 200, oldest first.
    pub fn chat_messages(
        &self,
        account_hex: &str,
        chat_id: &str,
    ) -> Result<Vec<TextMessage>, MessagingError> {
        let page = self.runtime.timeline_messages_with_query(
            account_hex,
            TimelineMessageQuery {
                group_id_hex: Some(chat_id.to_owned()),
                pagination: TimelinePagination {
                    limit: Some(HISTORY_LIMIT),
                    ..TimelinePagination::default()
                },
                ..TimelineMessageQuery::default()
            },
        )?;
        Ok(page.messages.iter().filter_map(text_message).collect())
    }

    /// The message `message_id` in the chat `chat_id`, if MDK has it as a text message.
    pub fn chat_message(
        &self,
        account_hex: &str,
        chat_id: &str,
        message_id: &str,
    ) -> Option<TextMessage> {
        let record = self
            .runtime
            .timeline_message(account_hex, chat_id, message_id)
            .ok()??;
        text_message(&record)
    }

    /// Follows the account's text messages in all its chats, from now on.
    pub async fn follow_messages(&self, account_hex: &str) -> Result<TextMessages, MessagingError> {
        let subscription = self
            .runtime
            .subscribe_timeline_messages(account_hex, TimelineMessageQuery::default())
            .await?;
        Ok(TextMessages {
            subscription,
            ready: VecDeque::new(),
        })
    }

    /// Follows the account's connections to its relays: the relays its relay lists name and the
    /// ones it was set up with. Relays that only a chat names aren't among them.
    pub async fn follow_relays(&self, account_hex: &str) -> Result<RelayLinks, MessagingError> {
        let subscription = self.runtime.subscribe_relay_liveness(account_hex).await?;
        Ok(RelayLinks { subscription })
    }

    /// Tells MDK that relays are back after an outage, so that messages waiting to go out are
    /// sent now rather than at their next retry.
    pub async fn connectivity_restored(&self) {
        if let Err(error) = self.runtime.notify_connectivity_restored().await {
            log::info!("could not hurry the messages waiting to go out: {error}");
        }
    }

    /// The secret both members of the chat `chat_id` hold in its current MLS epoch (#37).
    ///
    /// MDK lets the app read three exporter secrets of a chat, all of the current epoch; this is
    /// the one for group events. While the account has a change to the chat that the relays
    /// haven't confirmed yet, MDK gives the secret of the epoch that change starts but reports
    /// the epoch before it, so this reads only while the chat is settled, and fails if it doesn't
    /// settle within half a second. `docs/spikes/37-radio-keys.md` explains the choice.
    pub async fn chat_secret(
        &self,
        account_hex: &str,
        chat_id: &str,
    ) -> Result<ChatSecret, MessagingError> {
        let group_id = group_id(chat_id)?;
        let accounts = self.runtime.accounts();
        let settled =
            |state: &AppGroupMlsState| state.lifecycle_state == AppGroupLifecycleState::Stable;
        for attempt in 0..SECRET_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(SECRET_RETRY).await;
            }
            let before = self.runtime.group_mls_state(account_hex, &group_id).await?;
            if !settled(&before) {
                continue;
            }
            let secret = accounts
                .exporter_secret(account_hex, &group_id, GROUP_EVENT_EXPORTER, EXPORTER_BYTES)
                .await?;
            let after = self.runtime.group_mls_state(account_hex, &group_id).await?;
            if settled(&after) && before.epoch == after.epoch {
                return Ok(ChatSecret {
                    epoch: after.epoch,
                    secret,
                });
            }
        }
        Err(MessagingError::EpochUnsettled)
    }

    /// The account key as a copy to keep (#86): a NIP-49 `ncryptsec1…` protected by `password`,
    /// or the plain `nsec1…` without one.
    ///
    /// MDK writes every export to its audit log. A plain one also marks the key as handled
    /// insecurely, for good: protected copies made later carry that mark. A protected copy takes
    /// NIP-49's scrypt at log_n 18, about a second and 256 MiB, so call this off the async
    /// threads.
    pub fn export_key(
        &self,
        account_hex: &str,
        password: Option<&str>,
    ) -> Result<Zeroizing<String>, MessagingError> {
        Ok(match password {
            Some(password) => Zeroizing::new(self.runtime.export_encrypted_secret_key(
                account_hex,
                password,
                EXPORT_CALLER,
            )?),
            None => self.runtime.reveal_nsec(account_hex, EXPORT_CALLER)?,
        })
    }

    /// Updates the account's own key in the chat `chat_id`, which starts a new MLS epoch once
    /// the relays confirm it. MDK does this by itself every 24 to 36 days.
    pub async fn update_own_key(
        &self,
        account_hex: &str,
        chat_id: &str,
    ) -> Result<(), MessagingError> {
        self.runtime
            .schedule_manual_self_update(account_hex, &group_id(chat_id)?)
            .await?;
        // Scheduling only marks it as due; MDK's next maintenance pass would run it.
        self.runtime.run_due_maintenance(account_hex).await?;
        Ok(())
    }

    /// When this device first saw the direct chat `chat_id`, in milliseconds since the Unix
    /// epoch.
    pub fn chat_started_at(&self, account_hex: &str, chat_id: &str) -> Option<u64> {
        let row = self.runtime.chat_list_row(account_hex, chat_id).ok()??;
        // MDK stores this in seconds.
        Some(row.conversation_created_at.saturating_mul(1000)).filter(|time| *time > 0)
    }

    /// Stops the runtime and closes every database. Call it before the process may be
    /// suspended; afterwards this value cannot be used.
    pub async fn close(&self) -> Result<(), MessagingError> {
        self.runtime.shutdown_and_close().await?;
        Ok(())
    }
}

fn endpoints(relays: &[String]) -> Vec<TransportEndpoint> {
    relays.iter().cloned().map(TransportEndpoint).collect()
}

/// The MLS group id a chat id names.
fn group_id(chat_id: &str) -> Result<GroupId, MessagingError> {
    hex::decode(chat_id)
        .map(GroupId::new)
        .map_err(|_| MessagingError::InvalidChatId(chat_id.to_owned()))
}

/// A token unique to one send, which MDK uses to recognise a retried submission.
fn client_token() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{now:x}-{:x}", SENDS.fetch_add(1, Ordering::Relaxed))
}

/// The name a profile shows: its display name, else its name.
fn profile_name(profile: &UserProfileMetadata) -> Option<String> {
    [&profile.display_name, &profile.name]
        .into_iter()
        .flatten()
        .map(|name| name.trim())
        .find(|name| !name.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::canonical_relay;

    #[test]
    fn relays_are_spelled_one_way() {
        for (given, spelled) in [
            ("wss://relay.damus.io", "wss://relay.damus.io"),
            ("wss://relay.damus.io/", "wss://relay.damus.io"),
            ("  wss://nos.lol ", "wss://nos.lol"),
            ("WSS://Nos.Lol", "wss://nos.lol"),
            ("ws://127.0.0.1:7777/", "ws://127.0.0.1:7777"),
            ("wss://relay.example/nostr/", "wss://relay.example/nostr/"),
            ("not a relay", "not a relay"),
        ] {
            assert_eq!(canonical_relay(given), spelled, "{given}");
        }
    }
}
