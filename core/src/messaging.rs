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
use marmot_app::{
    AccountSetupReadiness, AccountSetupRequest, AppError, AppGroupLifecycleState, AppGroupMlsState,
    AppGroupRecord, MarmotApp, MarmotAppConfig, MarmotAppRuntime, MarmotDeployment, RelayLiveness,
    RelayLivenessStatus, RetentionSweepStatus, RuntimeChatsSubscription,
    RuntimeRelayLivenessSubscription, RuntimeTimelineMessageUpdate,
    RuntimeTimelineMessagesSubscription, SelfMembership, TimelineMessageChange,
    TimelineMessageQuery, TimelineMessageRecord, TimelinePagination, TimelineRemoveReason,
    UserProfileMetadata,
};
use nostr::types::RelayUrl;

use crate::photo::MAX_PHOTO_BYTES;
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
/// The app's own kind for an invitation's token (#95), which the invited person's app sends
/// once, right after it starts the chat. MDK refuses the kinds it acts on itself; this isn't one.
/// White Noise shows a kind it doesn't know, for Android as a bubble of its raw content and for
/// iOS as an "Unsupported event" card (#123). That does no harm here: only the inviter's app,
/// which is peoples.chat, receives the token.
pub const INVITE_TOKEN_KIND: u64 = 9_910;
/// The app's own kind for a radio address record (#34), which each app sends in its direct
/// chats when it has a radio. Provisional, like 9910: Marmot assigns kinds only for adopted specs.
pub const RADIO_ADDRESS_KIND: u64 = 9_911;
/// The mark MDK puts on a message of ours that no relay would take.
const LOCAL_PUBLISH_FAILED: &str = "local_publish_failed";
/// Makes each send's client token unique within this process.
static SENDS: AtomicU64 = AtomicU64::new(0);

/// Relays for new accounts unless the app is built with others (#18). New accounts publish their
/// relay lists, profile and KeyPackage here.
///
/// These are public, general-purpose relays. Who runs the project's own relays is decided later
/// (#11). They don't make accounts made here findable from White Noise: it looks a contact's
/// relay lists up only on its own relays (#22), and that stays to be decided (#76).
pub const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
];

/// Where profile photos are uploaded (#94): White Noise's server for profile photos, the proof of
/// concept's choice on 22 November 2026. The project's own media server comes with #11.
pub const DEFAULT_MEDIA_SERVER: &str = "https://blossom.primal.net";

/// White Noise's relays (#113). White Noise reads its users' contacts' profiles from these. It
/// keeps a copy there of what it found when it first looked the person up. So each profile change
/// is copied here too, or White Noise users keep seeing the old name and photo.
pub const WHITE_NOISE_RELAYS: &[&str] = &[
    "wss://relay.us.whitenoise.chat",
    "wss://relay.eu.whitenoise.chat",
    "wss://whitenoise.nostrdev.com",
];

/// Media servers signed into each chat the core starts (#120): the two White Noise founds its
/// chats with, provisionally. Each takes opaque `application/octet-stream` uploads, which is
/// what encrypted media looks like. Who runs the project's own is decided later (#11).
pub const CHAT_MEDIA_SERVERS: &[&str] =
    &["https://blossom.divine.video", "https://blossom.ditto.pub"];

/// The name our KeyPackages carry in their NIP-89 `client` tag (#120), as White Noise's carry
/// its own. Display metadata only.
pub const CLIENT_NAME: &str = "peoples.chat";

/// Whether `relays` are the public [`DEFAULT_RELAYS`], in any order and however spelled.
fn on_default_relays(relays: &[String]) -> bool {
    relays.len() == DEFAULT_RELAYS.len()
        && relays
            .iter()
            .all(|relay| DEFAULT_RELAYS.contains(&canonical_relay(relay).as_str()))
}

/// `values` when the app is on the public [`DEFAULT_RELAYS`], else nothing: a build with relays
/// of its own, such as a test relay, stays off public servers.
fn only_on_defaults(relays: &[String], values: &[&str]) -> Vec<String> {
    if on_default_relays(relays) {
        values.iter().map(|value| (*value).to_owned()).collect()
    } else {
        Vec::new()
    }
}

/// The relays that get a copy of each profile change (#113). An app on the public
/// [`DEFAULT_RELAYS`] gets White Noise's. A build with relays of its own, such as a test relay,
/// gets none, so that it stays off public relays.
pub fn profile_copy_relays(relays: &[String]) -> Vec<String> {
    only_on_defaults(relays, WHITE_NOISE_RELAYS)
}

/// The media servers new chats get (#120): [`CHAT_MEDIA_SERVERS`] on the public
/// [`DEFAULT_RELAYS`], none for a build with relays of its own. A chat started without any can
/// never carry photos or files.
pub fn chat_media_servers(relays: &[String]) -> Vec<String> {
    only_on_defaults(relays, CHAT_MEDIA_SERVERS)
}

/// The relays people are also looked up on, besides the app's own (#120): White Noise's, on the
/// public [`DEFAULT_RELAYS`]. White Noise users publish their relay lists and KeyPackages there,
/// and may never have published anywhere else. A build with relays of its own gets none.
pub fn lookup_relays(relays: &[String]) -> Vec<String> {
    only_on_defaults(relays, WHITE_NOISE_RELAYS)
}

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
    /// Keeps account keys in a file in the data folder instead of the platform's key store. This
    /// is for development builds on computers (#115). A key moves between the two at the first
    /// start of a build that keeps keys somewhere other than the build before did.
    pub keys_in_file: bool,
    /// Allows relays (`ws://`) and media servers (`http://`) on the loopback interface, for tests
    /// and local development.
    pub allow_loopback: bool,
    /// The media server profile photos go to; `None` for [`DEFAULT_MEDIA_SERVER`] (#94).
    pub media_server: Option<String>,
    /// Relays that also get a copy of each profile change, usually [`profile_copy_relays`]
    /// (#113). MDK copies in the background, so a failed copy never fails the change. It copies
    /// nothing for an account on loopback relays.
    pub profile_copies: Vec<String>,
    /// Media servers signed into each chat this core starts, usually [`chat_media_servers`]
    /// (#120). MDK writes them into the chat's state when it founds the chat. With none, the chat
    /// carries no photos or files for anyone in it, for good: nothing can add them later.
    pub chat_media_servers: Vec<String>,
    /// Relays people are looked up on besides [`Self::relays`], usually [`lookup_relays`]
    /// (#120): their relay lists, and their KeyPackages when their lists name no relays.
    pub lookup_relays: Vec<String>,
}

/// What another account's profile says about it, as the relays have it now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContactProfile {
    pub name: Option<String>,
    /// The address of the account's photo.
    pub picture: Option<String>,
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

/// What a chat is now, by its state in MDK (#125).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatForm {
    /// No name and two accounts: the account and the contact.
    Direct,
    /// Only the account is left: the contact left, or deleted their account.
    Alone,
    /// The chat has a name, or more than two accounts. MDK no longer counts it as direct.
    Group,
}

/// A chat as MDK holds it. The core shows direct chats, and keeps following one by its id once
/// it stops being direct (#125).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectChat {
    /// The MLS group id, in hex. It stays the same for the chat's life.
    pub id: String,
    /// The other person's public key, in hex, while the chat has exactly two accounts. Empty
    /// otherwise: the core knows whom a chat that was direct is with.
    pub peer_hex: String,
    pub form: ChatForm,
    /// The other person invited the account, and it has not accepted yet.
    pub invitation: bool,
    /// The account is still in the chat: the chat is not archived, left or removed.
    pub active: bool,
    /// The relays the chat's messages go through, as [`canonical_relay`] spells them. Empty when
    /// the core made this value itself rather than reading it from MDK.
    pub relays: Vec<String>,
    /// After how many seconds new messages disappear, when someone in the chat turned
    /// disappearing messages on (#124). `None` when they are off, or when the core made this
    /// value itself.
    pub disappear_after: Option<u64>,
}

/// The account's chats as they were when following began, then each one that changes. Groups
/// come too: the core decides by id which of them it shows (#125).
pub struct DirectChats {
    account_hex: String,
    subscription: RuntimeChatsSubscription,
}

impl DirectChats {
    /// The chats when following began, invitations included.
    pub fn current(&self) -> Vec<DirectChat> {
        self.subscription
            .snapshot
            .iter()
            .map(|record| direct_chat(&self.account_hex, record))
            .collect()
    }

    /// The next chat that was added or changed. `None` once messaging has stopped.
    pub async fn next(&mut self) -> Option<DirectChat> {
        let record = self.subscription.recv().await?;
        Some(direct_chat(&self.account_hex, &record))
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
    /// Who sent it: their public key, in hex (#125).
    pub sender_hex: String,
    pub text: String,
    /// When the sender sent it, by the sender's clock, in milliseconds since the Unix epoch. MDK
    /// keeps whole seconds.
    pub sent_at: u64,
    /// Outgoing only: a relay has acknowledged it.
    pub acknowledged: bool,
    /// When it disappears, in milliseconds since the Unix epoch (whole seconds), if it was sent
    /// while the chat's messages disappear (#124). MDK pins it to the message: the sender's
    /// clock when it was sent, plus the chat's timer then.
    pub expires_at: Option<u64>,
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
        sender_hex: record.sender.clone(),
        text: record.plaintext.clone(),
        sent_at: record.timeline_at.saturating_mul(1000),
        acknowledged: outgoing
            && (record.source_message_id_hex.is_some() || record.recipient_confirmed),
        expires_at: record
            .retention_expires_at
            .map(|seconds| seconds.saturating_mul(1000)),
    })
}

/// An invitation's token that the other person sent in a chat (#95).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InviteToken {
    pub chat_id: String,
    pub token: String,
}

/// The invitation's token in `record`, if it is one the other person sent.
fn invite_token(record: &TimelineMessageRecord) -> Option<InviteToken> {
    (record.kind == INVITE_TOKEN_KIND && record.direction == "received" && !record.deleted).then(
        || InviteToken {
            chat_id: record.group_id_hex.clone(),
            token: record.plaintext.trim().to_owned(),
        },
    )
}

/// A radio address record that the other person sent in a chat (#34), as it came: the core
/// reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioAddressItem {
    pub chat_id: String,
    pub record: String,
}

/// The radio address record in `record`, if it is one the other person sent.
fn radio_address(record: &TimelineMessageRecord) -> Option<RadioAddressItem> {
    (record.kind == RADIO_ADDRESS_KIND && record.direction == "received" && !record.deleted).then(
        || RadioAddressItem {
            chat_id: record.group_id_hex.clone(),
            record: record.plaintext.clone(),
        },
    )
}

/// What can come in a chat, as the core follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatItem {
    Text(TextMessage),
    InviteToken(InviteToken),
    RadioAddress(RadioAddressItem),
    /// MDK deleted the message `message_id` because its time was up (#124).
    Expired {
        message_id: String,
    },
}

fn chat_item(record: &TimelineMessageRecord) -> Option<ChatItem> {
    text_message(record)
        .map(ChatItem::Text)
        .or_else(|| invite_token(record).map(ChatItem::InviteToken))
        .or_else(|| radio_address(record).map(ChatItem::RadioAddress))
}

/// What comes in the account's chats as it is added or changes: text messages, for example
/// when a relay acknowledges one of ours, and invitations' tokens.
pub struct ChatItems {
    subscription: RuntimeTimelineMessagesSubscription,
    ready: VecDeque<ChatItem>,
}

impl ChatItems {
    /// The next item that was added or changed. `None` once messaging has stopped.
    pub async fn next(&mut self) -> Option<ChatItem> {
        loop {
            if let Some(item) = self.ready.pop_front() {
                return Some(item);
            }
            match self.subscription.recv().await? {
                // MDK fell behind and sends its latest messages again.
                RuntimeTimelineMessageUpdate::Page { page } => {
                    self.ready
                        .extend(page.messages.iter().filter_map(chat_item));
                }
                RuntimeTimelineMessageUpdate::Projection(projection) => {
                    for change in &projection.update.timeline_changes {
                        match change {
                            TimelineMessageChange::Upsert { message, .. } => {
                                self.ready.extend(chat_item(message));
                            }
                            // The retention sweep deleted it (#124). Deletions are #122's.
                            TimelineMessageChange::Remove {
                                message_id_hex,
                                reason: TimelineRemoveReason::Pruned,
                            } => self.ready.push_back(ChatItem::Expired {
                                message_id: message_id_hex.clone(),
                            }),
                            TimelineMessageChange::Remove { .. } => {}
                        }
                    }
                }
            }
        }
    }
}

/// The chat in `record`, and what it is now (#125). The account's own device group, of one
/// account, reads as [`ChatForm::Alone`]; the core never showed it, so it ignores it.
fn direct_chat(account_hex: &str, record: &AppGroupRecord) -> DirectChat {
    // The other account of a two-account roster, named or not.
    let peer = |members: &Option<Vec<String>>| {
        let members = members.as_ref()?;
        if members.len() != 2 || !members.iter().any(|member| member == account_hex) {
            return None;
        }
        members
            .iter()
            .find(|member| *member != account_hex)
            .cloned()
    };
    let (form, peer_hex) = if let Some(peer_hex) = peer(&record.direct_member_ids_hex) {
        (ChatForm::Direct, peer_hex)
    } else if record.member_count == Some(1) {
        (ChatForm::Alone, String::new())
    } else {
        (
            ChatForm::Group,
            peer(&record.presentation_member_ids_hex).unwrap_or_default(),
        )
    };
    DirectChat {
        id: record.group_id_hex.clone(),
        peer_hex,
        form,
        invitation: record.pending_confirmation,
        active: !record.archived && matches!(record.self_membership, SelfMembership::Member),
        relays: record
            .nostr_routing
            .relays
            .iter()
            .map(|relay| canonical_relay(relay))
            .collect(),
        // Marmot: 0 means off (message-retention-v1).
        disappear_after: Some(record.message_retention.disappearing_message_secs)
            .filter(|secs| *secs > 0),
    }
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
    #[error("could not download the photo: {0}")]
    PhotoDownload(String),
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
    media_server: String,
    allow_loopback: bool,
    profile_copies: Vec<String>,
}

impl Messaging {
    /// Opens the stores in the data folder and starts the runtime. Account keys live in the
    /// platform's key store. Must run inside a Tokio runtime, which MDK's relay tasks use.
    pub async fn open(options: MessagingOptions) -> Result<Self, MessagingError> {
        let home = crate::keys::open_account_home(
            &options.data_dir,
            &options.keychain_service,
            options.keys_in_file,
        )
        .map_err(AppError::from)?;
        let config = MarmotAppConfig::default()
            .with_deployment(MarmotDeployment {
                new_group_media_endpoints: options.chat_media_servers.clone(),
                ..MarmotDeployment::neutral()
            })
            .with_key_package_client_name(Some(CLIENT_NAME.to_owned()))
            .with_directory_relay_urls(directory_relays(&options.relays, &options.lookup_relays))
            .with_allow_loopback_relay_endpoints(options.allow_loopback)
            .with_allow_loopback_blob_endpoints(options.allow_loopback);
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
            media_server: options
                .media_server
                .unwrap_or_else(|| DEFAULT_MEDIA_SERVER.to_owned()),
            allow_loopback: options.allow_loopback,
            profile_copies: options.profile_copies,
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

    /// Signs the account out and removes it from this device: its databases and its key, from the
    /// keychain or its file (#115). Telling relays and groups is best effort and does not block the
    /// local wipe.
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

    /// The media servers signed into the chat's state (#120), where photos and files in it go.
    /// Empty when the chat has no encrypted-media component, so it can never carry them.
    pub async fn media_servers_in_chat(
        &self,
        account_hex: &str,
        chat_id: &str,
    ) -> Result<Vec<String>, MessagingError> {
        let snapshot = self
            .runtime
            .group_conversation_snapshot(account_hex, &group_id(chat_id)?)
            .await?;
        let media = snapshot.group.encrypted_media;
        if !media.required {
            return Ok(Vec::new());
        }
        Ok(media
            .default_blob_endpoints
            .into_iter()
            .map(|endpoint| endpoint.base_url)
            .collect())
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
        match self
            .fetch_profile(account_hex, relay_hints)
            .await
            .and_then(|profile| profile.name)
        {
            Some(name) => name,
            None => marmot_app::default_profile_pseudonym(account_hex),
        }
    }

    /// Another account's profile as the relays have it now, from `relay_hints` and our relays.
    /// MDK keeps what it finds. `None` when there is no profile, or the relays didn't answer in
    /// time.
    pub async fn fetch_profile(
        &self,
        account_hex: &str,
        relay_hints: &[String],
    ) -> Option<ContactProfile> {
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
                return Some(ContactProfile {
                    name: profile_name(&profile),
                    picture: profile_picture(&profile),
                });
            }
            Ok(Ok(None)) => log::info!("the contact has no profile on the relays"),
            Ok(Err(error)) => log::info!("could not fetch the contact's profile: {error}"),
            Err(_) => log::info!("fetching the contact's profile timed out"),
        }
        None
    }

    /// What the account's own profile says about it, as this device last saw it (#93).
    pub fn own_about(&self, account_hex: &str) -> String {
        self.cached_profile(account_hex)
            .and_then(|profile| profile.about)
            .unwrap_or_default()
    }

    /// The address of an account's photo, as this device last saw its profile (#94).
    pub fn known_picture(&self, account_hex: &str) -> Option<String> {
        self.cached_profile(account_hex)
            .and_then(|profile| profile_picture(&profile))
    }

    /// Publishes the account's profile with a new name and about (#93).
    pub async fn update_profile(
        &self,
        account_hex: &str,
        name: &str,
        about: &str,
    ) -> Result<(), MessagingError> {
        self.publish_profile(account_hex, |profile| {
            profile.name = Some(name.to_owned());
            profile.display_name = Some(name.to_owned());
            profile.about = (!about.is_empty()).then(|| about.to_owned());
        })
        .await
    }

    /// Publishes the account's profile with `picture` as its photo's address, or without a photo
    /// (#94).
    pub async fn set_picture(
        &self,
        account_hex: &str,
        picture: Option<&str>,
    ) -> Result<(), MessagingError> {
        self.publish_profile(account_hex, |profile| {
            profile.picture = picture.map(str::to_owned);
        })
        .await
    }

    /// Publishes the account's profile with `change` made to it. Everything else in the profile
    /// stays as it was, including what other apps put there: MDK replaces name, about, picture,
    /// NIP-05 and lightning address with whatever it is given, so they come from the profile as
    /// this device has it.
    ///
    /// It goes to the account's relays, and a copy goes to the profile copy relays (#113). The
    /// change counts once an account relay has it. MDK sends the copies afterwards, in the
    /// background.
    async fn publish_profile(
        &self,
        account_hex: &str,
        change: impl FnOnce(&mut UserProfileMetadata),
    ) -> Result<(), MessagingError> {
        let mut profile = self.cached_profile(account_hex).unwrap_or_default();
        change(&mut profile);
        self.runtime
            .publish_user_profile_using_account_relays_and_indexers(
                account_hex,
                profile,
                endpoints(&self.profile_copies),
            )
            .await?;
        Ok(())
    }

    /// Uploads a profile photo to the media server and returns its address (#94). The photo is a
    /// JPEG the core made, without metadata: MDK uploads what it is given.
    pub async fn upload_photo(
        &self,
        account_hex: &str,
        jpeg: Vec<u8>,
    ) -> Result<String, MessagingError> {
        Ok(self
            .runtime
            .upload_profile_image(account_hex, jpeg, "image/jpeg", Some(&self.media_server))
            .await?)
    }

    /// Downloads the photo at `url`, up to [`MAX_PHOTO_BYTES`] (#94). MDK's download takes public
    /// HTTPS addresses only. With loopback allowed, a media server on this machine is fetched
    /// directly, for tests and local development.
    pub async fn download_photo(&self, url: &str) -> Result<Vec<u8>, MessagingError> {
        if self.allow_loopback && is_loopback_http(url) {
            return fetch_loopback(url).await;
        }
        Ok(marmot_app::download_profile_image(url.to_owned(), MAX_PHOTO_BYTES as u64).await?)
    }

    /// An account's profile as this device last saw it.
    fn cached_profile(&self, account_hex: &str) -> Option<UserProfileMetadata> {
        match self
            .app
            .directory()
            .directory_entry_for_account_id(account_hex)
        {
            Ok(entry) => entry.and_then(|entry| entry.profile),
            Err(error) => {
                log::warn!("could not read a profile from this device: {error}");
                None
            }
        }
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

    /// The invitations' tokens the other person sent in the chat `chat_id` (#95).
    pub fn invite_tokens(
        &self,
        account_hex: &str,
        chat_id: &str,
    ) -> Result<Vec<InviteToken>, MessagingError> {
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
        Ok(page.messages.iter().filter_map(invite_token).collect())
    }

    /// The radio address records the other person sent in the chat `chat_id` (#34), oldest
    /// first.
    pub fn radio_addresses(
        &self,
        account_hex: &str,
        chat_id: &str,
    ) -> Result<Vec<RadioAddressItem>, MessagingError> {
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
        Ok(page.messages.iter().filter_map(radio_address).collect())
    }

    /// Sends this phone's radio address record in the chat `chat_id` (#34).
    pub async fn send_radio_address(
        &self,
        account_hex: &str,
        chat_id: &str,
        record: &str,
    ) -> Result<(), MessagingError> {
        self.runtime
            .send_custom_event(
                account_hex,
                &group_id(chat_id)?,
                RADIO_ADDRESS_KIND,
                Vec::new(),
                record.to_owned(),
            )
            .await?;
        Ok(())
    }

    /// Sends an invitation's token in the chat `chat_id`, so that the inviter's app opens the
    /// chat without a request (#95).
    pub async fn send_invite_token(
        &self,
        account_hex: &str,
        chat_id: &str,
        token: &str,
    ) -> Result<(), MessagingError> {
        self.runtime
            .send_custom_event(
                account_hex,
                &group_id(chat_id)?,
                INVITE_TOKEN_KIND,
                Vec::new(),
                token.to_owned(),
            )
            .await?;
        Ok(())
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

    /// Marks everything in the chat `chat_id` as read, up to its newest message (#124). MDK's
    /// read cursor stays on this device: nothing is sent. The retention sweep deletes only
    /// expired messages that are read.
    pub fn mark_chat_read(&self, account_hex: &str, chat_id: &str) -> Result<(), MessagingError> {
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
        // The newest one received: the user's own message, sent in the same second, may come
        // after it or before it, and MDK's read mark counts from the one it is given.
        let newest = page
            .messages
            .iter()
            .rev()
            .find(|record| record.direction == "received");
        if let Some(newest) = newest {
            self.runtime.mark_timeline_message_read(
                account_hex,
                chat_id,
                &newest.message_id_hex,
            )?;
        }
        Ok(())
    }

    /// Whether the chat `chat_id` has messages the user hasn't seen, by MDK's read mark (#124).
    pub fn has_unread(&self, account_hex: &str, chat_id: &str) -> Result<bool, MessagingError> {
        Ok(self
            .runtime
            .chat_list_row(account_hex, chat_id)?
            .is_some_and(|row| row.has_unread))
    }

    /// Has MDK delete the plaintext of expired disappearing messages in all the account's
    /// chats, as of `now_ms` (#124). MDK holds back a chat while an expired message in it is
    /// unread, or while one expired less than 5 seconds ago. Returns how many it deleted; each
    /// also comes as [`ChatItem::Expired`].
    pub async fn sweep_expired(
        &self,
        account_hex: &str,
        now_ms: u64,
    ) -> Result<u64, MessagingError> {
        let report = self
            .runtime
            .sweep_expired_retention(account_hex, now_ms)
            .await?;
        for group in &report.groups {
            if group.status == RetentionSweepStatus::Failed {
                log::warn!(
                    "deleting a chat's expired messages failed: {}",
                    group.failure_kind.as_deref().unwrap_or("unknown")
                );
            }
        }
        Ok(report
            .groups
            .iter()
            .map(|group| group.pruned_messages)
            .sum())
    }

    /// Follows the account's text messages in all its chats, from now on.
    pub async fn follow_messages(&self, account_hex: &str) -> Result<ChatItems, MessagingError> {
        let subscription = self
            .runtime
            .subscribe_timeline_messages(account_hex, TimelineMessageQuery::default())
            .await?;
        Ok(ChatItems {
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

    /// Gives the chat `chat_id` a name, which makes it a group for MDK and for every member's
    /// app (#125). The interface doesn't offer this; tests use it to do what another app can.
    pub async fn rename_chat(
        &self,
        account_hex: &str,
        chat_id: &str,
        name: &str,
    ) -> Result<(), MessagingError> {
        self.runtime
            .update_group_profile(
                account_hex,
                &group_id(chat_id)?,
                Some(name.to_owned()),
                None,
            )
            .await?;
        Ok(())
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

/// The relays MDK looks people up on: ours, then `lookup`, each once. MDK uses these instead of
/// ours, not besides them, so ours come first. With no `lookup` relays, none: MDK then looks on
/// ours, as before #120.
fn directory_relays(relays: &[String], lookup: &[String]) -> Vec<String> {
    if lookup.is_empty() {
        return Vec::new();
    }
    let mut all: Vec<String> = Vec::new();
    for relay in relays.iter().chain(lookup) {
        let relay = canonical_relay(relay);
        if !all.contains(&relay) {
            all.push(relay);
        }
    }
    all
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
fn profile_picture(profile: &UserProfileMetadata) -> Option<String> {
    profile
        .picture
        .as_deref()
        .map(str::trim)
        .filter(|picture| !picture.is_empty())
        .map(str::to_owned)
}

/// Whether `url` is plain HTTP on this machine.
fn is_loopback_http(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default();
    url.scheme() == "http"
        && (host == "localhost"
            || host
                .trim_matches(|c| c == '[' || c == ']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback()))
}

async fn fetch_loopback(url: &str) -> Result<Vec<u8>, MessagingError> {
    let failed = |error: reqwest::Error| MessagingError::PhotoDownload(error.to_string());
    let response = reqwest::Client::new()
        .get(url)
        .timeout(PROFILE_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?;
    let bytes = response.bytes().await.map_err(failed)?;
    if bytes.len() > MAX_PHOTO_BYTES {
        return Err(MessagingError::PhotoDownload(
            "the photo is too large".into(),
        ));
    }
    Ok(bytes.to_vec())
}

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
    use super::{
        ChatForm, DEFAULT_RELAYS, WHITE_NOISE_RELAYS, canonical_relay, chat_media_servers,
        direct_chat, directory_relays, lookup_relays, profile_copy_relays,
    };
    use marmot_app::{AppGroupRecord, SelfMembership};

    /// A chat's record as MDK v0.18.0 gave it in `issue_125_chat_stops_being_direct`, once the
    /// contact had left: one account, no name.
    const LEFT_RECORD: &str = r#"{"group_id_hex":"7f1f288b40fd8c00c2379118eda153a8","protocol_profile":"legacy","endpoint":"ws://127.0.0.1:50947","nostr_routing":{"component_id":32772,"component":"marmot.transport.nostr.routing.v1","nostr_group_id_hex":"000387dd80dddf903670887bbe0a79c819cd39546f38dfc4ee265ab106dd0fad","relays":["ws://127.0.0.1:50947"],"data_hex":"000387dd80dddf903670887bbe0a79c819cd39546f38dfc4ee265ab106dd0fad151477733a2f2f3132372e302e302e313a3530393437"},"nostr_routing_last_epoch":3,"prior_nostr_routes":[],"profile":{"component_id":32769,"component":"marmot.group.profile.v1","present":true,"name":"","description":"","data_hex":"0000"},"image":{"component_id":32770,"component":"marmot.group.blossom.image.v1","present":false,"image_hash_hex":"","image_key_hex":"","image_nonce_hex":"","image_upload_key_hex":"","media_type":null,"data_hex":"0000000000"},"avatar_url":{"component_id":32775,"component":"marmot.group.avatar-url.v1","present":false,"url":"","dim":null,"thumbhash":null,"data_hex":"000000"},"admin_policy":{"component_id":32771,"component":"marmot.group.admin-policy.v1","admins":["ac762ca44a352c9b60bfd54060121ed977e31b92971a1fe765a559335630fc8a"],"data_hex":"20ac762ca44a352c9b60bfd54060121ed977e31b92971a1fe765a559335630fc8a"},"message_retention":{"component_id":32773,"component":"marmot.group.message-retention.v1","disappearing_message_secs":0,"data_hex":"0000000000000000"},"agent_text_stream":{"component_id":32774,"component":"marmot.group.agent-text-stream.quic.v1","required":true,"required_member_roles":["receive"],"allowed_member_roles":["receive","send"],"max_plaintext_frame_len":4096,"replay_ttl_secs":0,"padding_bucket_bytes":0,"data_hex":"010300001000000000000000"},"encrypted_media":{"component_id":32776,"component":"marmot.group.encrypted-media.v1","required":false,"media_format":"","allowed_locator_kinds":[],"default_blob_endpoints":[],"data_hex":""},"unknown_components":[],"archived":false,"pending_confirmation":false,"member_count":1,"self_membership":"Member","disbanding":false,"disbanded":false,"unrecoverable":false}"#;

    /// #125: what a chat is, from MDK's record of it.
    #[test]
    fn issue_125_a_chat_is_direct_alone_or_a_group() {
        let me = "a".repeat(64);
        let bea = "b".repeat(64);
        let carl = "c".repeat(64);
        let left: AppGroupRecord = serde_json::from_str(LEFT_RECORD).unwrap();

        // No name and two accounts: direct, with the other one.
        let mut direct = left.clone();
        direct.member_count = Some(2);
        direct.direct_member_ids_hex = Some(vec![me.clone(), bea.clone()]);
        direct.presentation_member_ids_hex = direct.direct_member_ids_hex.clone();
        let chat = direct_chat(&me, &direct);
        assert_eq!(
            (chat.form, chat.peer_hex.as_str()),
            (ChatForm::Direct, &*bea)
        );
        assert!(chat.active);
        assert_eq!(chat.relays, ["ws://127.0.0.1:50947"]);

        // The contact left: only the account is in it.
        let chat = direct_chat(&me, &left);
        assert_eq!((chat.form, chat.peer_hex.as_str()), (ChatForm::Alone, ""));
        assert!(chat.active);

        // A name, with the same two accounts: MDK no longer counts it as direct.
        let mut named = direct.clone();
        named.profile.name = "Allotment".to_owned();
        named.direct_member_ids_hex = None;
        let chat = direct_chat(&me, &named);
        assert_eq!(
            (chat.form, chat.peer_hex.as_str()),
            (ChatForm::Group, &*bea)
        );

        // A third account.
        let mut three = left.clone();
        three.member_count = Some(3);
        let chat = direct_chat(&me, &three);
        assert_eq!((chat.form, chat.peer_hex.as_str()), (ChatForm::Group, ""));

        // Two accounts, and the account isn't one of them: not a chat with it.
        let mut others = direct.clone();
        others.direct_member_ids_hex = Some(vec![bea.clone(), carl]);
        assert_eq!(direct_chat(&me, &others).form, ChatForm::Group);

        // The account was removed, or left: no longer active.
        let mut removed = direct;
        removed.self_membership = SelfMembership::Removed;
        let chat = direct_chat(&me, &removed);
        assert_eq!(chat.form, ChatForm::Direct);
        assert!(!chat.active);
    }

    #[test]
    fn profiles_are_copied_to_white_noise_only_from_the_default_relays() {
        let owned = |relays: &[&str]| relays.iter().map(|r| (*r).to_owned()).collect::<Vec<_>>();
        let white_noise = owned(WHITE_NOISE_RELAYS);
        assert_eq!(profile_copy_relays(&owned(DEFAULT_RELAYS)), white_noise);
        // In any order, and however they are spelled.
        assert_eq!(
            profile_copy_relays(&owned(&[
                "wss://relay.primal.net/",
                "WSS://nos.lol",
                "wss://relay.damus.io"
            ])),
            white_noise
        );
        // A build with relays of its own copies nothing.
        for relays in [
            &["ws://127.0.0.1:7777"][..],
            &["ws://10.0.2.2:7777"],
            &["wss://relay.peoples.chat"],
            &["wss://nos.lol", "wss://relay.primal.net"],
            &[
                "wss://relay.damus.io",
                "wss://nos.lol",
                "wss://relay.primal.net",
                "wss://relay.peoples.chat",
            ],
            &[],
        ] {
            assert!(profile_copy_relays(&owned(relays)).is_empty(), "{relays:?}");
        }
    }

    /// #120: on the defaults, new chats get White Noise's media servers and people are also looked
    /// up on White Noise's relays. A build with relays of its own stays off public servers.
    #[test]
    fn issue_120_only_the_default_relays_use_public_media_servers_and_lookup_relays() {
        let owned = |relays: &[&str]| relays.iter().map(|r| (*r).to_owned()).collect::<Vec<_>>();
        let defaults = owned(DEFAULT_RELAYS);
        assert_eq!(
            chat_media_servers(&defaults),
            ["https://blossom.divine.video", "https://blossom.ditto.pub"]
        );
        assert_eq!(lookup_relays(&defaults), owned(WHITE_NOISE_RELAYS));
        for relays in [
            &["ws://127.0.0.1:7777"][..],
            &["wss://relay.peoples.chat"],
            &["wss://nos.lol", "wss://relay.primal.net"],
            &[],
        ] {
            assert!(chat_media_servers(&owned(relays)).is_empty(), "{relays:?}");
            assert!(lookup_relays(&owned(relays)).is_empty(), "{relays:?}");
        }
    }

    /// MDK looks people up on the directory relays instead of ours, so ours are listed first.
    #[test]
    fn issue_120_people_are_looked_up_on_our_relays_and_the_lookup_relays() {
        let owned = |relays: &[&str]| relays.iter().map(|r| (*r).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            directory_relays(
                &owned(&["wss://nos.lol/", "wss://relay.damus.io"]),
                &owned(&["wss://relay.eu.whitenoise.chat", "wss://nos.lol"]),
            ),
            [
                "wss://nos.lol",
                "wss://relay.damus.io",
                "wss://relay.eu.whitenoise.chat"
            ]
        );
        // Without lookup relays MDK looks on ours, as it always has.
        assert!(directory_relays(&owned(&["wss://nos.lol"]), &[]).is_empty());
    }

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
