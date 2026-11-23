# MDK in the core

The core uses [MDK](https://github.com/marmot-protocol/mdk), the Marmot Development Kit, for Marmot messaging over Nostr
relays. It depends on MDK's multi-account app runtime, the `marmot-app` crate, directly from Rust.

MDK documents two host surfaces: the MarmotKit bindings (Swift and Kotlin) and the C ABI. A Rust host using `marmot-app`
directly is not among them, but it works, so we use it. If it breaks, the fallbacks are MDK's C ABI (`marmot-c`) or the
lower `marmot-session` crate.

## What the core uses

`core/src/messaging.rs` wraps MDK's runtime in a small `Messaging` type:

| Core | MDK |
| --- | --- |
| `Messaging::open` | `AccountHome::open_with_secret_store` with `KeychainSecretStore` or `LocalFileSecretStore` (#115), `MarmotApp::try_with_relays_and_account_home_and_config`, `MarmotAppRuntime::start` |
| `create_account(name)` | `create_identity_local_ready_with_initial_profile` |
| `accounts()` | `AccountManager::managed_accounts`, `display_name_for_account_id` |
| `delete_account` | `sign_out_and_wipe` |
| `wait_until_published` | polls `account_setup_readiness` until `NetworkReady` |
| `start_direct_chat` | `Directory::fetch_account_relay_list_status_for_account_id` (relay hints), `start_direct_conversation` |
| `accept_direct_chat` | `accept_group_invite` |
| `follow_direct_chats` | `subscribe_chats` |
| `known_name`, `contact_name`, `fetch_name` | `display_name_for_account_id`, `fetch_current_user_profile_for_account_id`, `default_profile_pseudonym` |
| `own_about`, `known_picture`, `update_profile`, `set_picture` | `MarmotApp::directory().directory_entry_for_account_id` (the cached profile), `publish_user_profile_using_account_relays` |
| `upload_photo`, `download_photo` | `upload_profile_image`, `download_profile_image` |
| `send_invite_token`, `invite_tokens`, `follow_messages` (tokens) | `send_custom_event`, `timeline_messages_with_query`, `subscribe_timeline_messages` |
| `chat_started_at` | `chat_list_row` |
| `send_text` | `submit_text` |
| `chat_messages`, `chat_message` | `timeline_messages_with_query`, `timeline_message` |
| `follow_messages` | `subscribe_timeline_messages` |
| `follow_relays` | `subscribe_relay_liveness` |
| `connectivity_restored` | `notify_connectivity_restored` |
| `chat_secret` | `group_mls_state`, `AccountManager::exporter_secret` (`marmot/group-event`) |
| `update_own_key` | `schedule_manual_self_update`, `run_due_maintenance` |
| `export_key` | `export_encrypted_secret_key` (NIP-49), `reveal_nsec` |
| `close` | `shutdown_and_close` |

Invite links (`core/src/invite.rs`) use `nprofile_for_account_id`, `account_id_hex_from_ref` and `is_nostr_secret`, and
read relay hints with the `nostr` crate that MDK pins.

`Core` (`core/src/app.rs`) builds the interface's commands and events on it; see
[core-interface.md](core-interface.md).

Choices:

- **Keys in the platform's key store**, one entry per account under the service name the app passes in. MDK's other
  store, a plain-text key file, is meant for development and is not used. See [The account key](#the-account-key).
- **Neutral configuration** (`MarmotAppConfig::default()`): no public indexers and no ranking service, unlike White
  Noise's configuration.
- **The profile name from the start.** `create_identity` would first publish a pseudonym derived from the key; the
  initial-profile variant publishes the user's name instead.
- **Account setup in two steps.** The account exists locally as soon as `create_account` returns. Publishing its relay
  lists (kinds 10002 and 10050), an empty follow list (kind 3), its profile (kind 0) and its KeyPackage (kind 30443)
  continues in the background, and each needs a relay's acknowledgement.
- **Relays are required.** MDK refuses to set up an account without default relays (`MissingDefaultRelays`), but they
  don't need to be reachable. See [Relays](#relays) for the ones we use.
- **Deleting wipes.** `sign_out_and_wipe` removes the account's databases and its key. Telling relays and groups is
  best effort and never blocks the local wipe. On the relays, the deletion request removed the KeyPackage; the profile,
  follow list and relay lists stayed.

## The account key

The spec's rule (Identity and keys): the app reaches the account key only through a signer interface, so external
signers and threshold signing stay possible. MDK provides that interface, and the core adds none of its own. No code of
ours reads, holds or passes the key, except to export it when the user asks (below).

**Where it lives.** The account home keeps one entry per account in the platform's key store (MDK's
`KeychainSecretStore`):

- the service is the app's `chat.peoples.client`;
- the user is the account's public key, in hex;
- the secret is the secret key, in hex.

| Platform | Store | Notes |
| --- | --- | --- |
| macOS | Login keychain | |
| iOS | Data-protection keychain | The service is `chat.peoples.client.after-first-unlock-v1`. The item is `AfterFirstUnlockThisDeviceOnly`: background work can read it once the phone has been unlocked since it was switched on, and it never moves to another phone, not even through a backup. |
| Android | SharedPreferences, encrypted with an AES-GCM key in Android Keystore | The Keystore key needs no user authentication, so the key works while the phone is locked. As on iOS, that holds once the phone has been unlocked since it was switched on; until then Android doesn't start the app. Tauri provides the Android context the store needs. |
| Linux, Windows | Secret Service, Credential Manager | |
| Tests | `keyring-core`'s in-memory store | Never the machine's own keychain. |

**Development builds on computers keep the key in a file** (#115):

- **Why.** The Mac's keychain sees each rebuild as a new app. It asks for the login password at every start, and
  "Always Allow" doesn't last.
- **Where.** Such builds use MDK's `LocalFileSecretStore`: `accounts/<label>/secret.json` in the data folder, in plain
  text. It is fine for test accounts, but a development build shouldn't hold an account that matters.
- **Turning it off.** Build with `TAURI_PEOPLES_CHAT_KEYCHAIN=1` to keep the keychain.
- **Not affected.** Release builds and phones always use the platform's store.
- **One place at a time.** A key moves at the first start of a build that keeps keys somewhere other than the build
  before did: into the file, or back into the keychain (`core/src/keys.rs`).
  - Only the file is asked where a key is, so a development build reads the keychain only to move a key out. That
    is one last prompt.
  - A key that can't move, because the prompt was declined, stays where it is, and the app uses that place for the
    session.
  - If the old copy can't be removed once the key has moved, it stays behind, and the log says so.
- **Release builds on Macs** will move to the data-protection keychain, which never asks. That needs the signed app
  (#61).

**Signing.**

- When MDK starts an account, it loads the key from the store and signs with it in memory (`AccountSigner::Local` in
  `marmot-app`).
- MDK also derives the keys of the account's encrypted databases from it. Losing the key loses the local history too.

**External signers.**

- An account whose key never enters MDK uses a signer the host provides, implementing `marmot_app::ExternalAccountSigner`.
  It covers Nostr signing, NIP-04 and NIP-44 encryption, and Marmot's account identity proof.
- `login_external_signer` adds such an account. After every start, `register_external_signer` connects its signer again.
- NIP-46 remote signers are built into MDK.
- A signer app (NIP-55) or threshold signing would plug in here without changing the interface. Neither is part of the
  PoC.

**Exporting it** (#86; spec v0.2, Key export). `Core::export_account_key` returns a copy the user keeps:

- **A protected copy:** with a password of at least 12 characters, MDK's `export_encrypted_secret_key` makes a NIP-49
  `ncryptsec1…`, with scrypt at log_n 18 (about a second and 256 MiB). Other Nostr apps and signers open it with the
  password.
- **The plain key:** without a password, MDK's `reveal_nsec` gives the `nsec1…`. MDK then marks the key as handled
  insecurely, for good, and every protected copy made later carries that mark (NIP-49's key security byte: weak
  instead of unknown).
- **The audit log:** MDK writes every export to the account's `audit-key-reveal.jsonl`, with the label
  `peoples_chat_core::Core::export_account_key` and no key material.
- **The device unlock comes first:** Face ID, Touch ID or a fingerprint, or the device passcode. The app shell asks for it
  through `tauri-plugin-biometric`, so the web view can't skip it. Linux and Windows have no unlock there, so
  exporting fails on them.

**What the web view sees:** the public key and the invite link. The key itself only when the user exports it, and only
on the screen that shows it.

**Not checked yet:** keychain access in a signed iPhone app, and Android's hardware-backed keystore on a real phone (#12).

## Relays

**Defaults.** New accounts use three public, general-purpose relays: `wss://relay.damus.io`, `wss://nos.lol` and
`wss://relay.primal.net` (`DEFAULT_RELAYS` in `core/src/messaging.rs`). Who runs the project's own relays is decided
later (#11). Building with `TAURI_PEOPLES_CHAT_RELAYS` (comma-separated) replaces them; see
[core-interface.md](core-interface.md#running-the-app-with-the-real-core).

**How other clients find an account.**

1. They look up its relay lists (kinds 10002 and 10050) on discovery relays of their own.
2. They fetch its KeyPackage (kind 30443) from the relays the list names.

White Noise's discovery relays are `purplepag.es`, `relay.vertexlab.io`, `nos.lol`, `relay.ditto.pub` and
`relay.primal.net`. When we checked on 22 November 2026:

- `nos.lol`, `relay.ditto.pub` and `relay.primal.net` found a new account and its KeyPackage. Ditto found it even
  though we don't publish there.
- `purplepag.es` and `relay.vertexlab.io` didn't.
- `relay.damus.io` answered WebSocket connections with HTTP 503 that day. The account's events reached the other two
  relays only.

**Checking an account with `wn`.** MDK's command-line client makes the same lookup.

1. Build it from the pinned tag. `ODIN` must point at the compiler from `scripts/install-odin.sh`:

   ```sh
   ODIN=$PWD/.tools/odin/odin cargo install --git https://github.com/marmot-protocol/mdk --tag v0.18.0 wn-cli \
     --bin wn --locked --debug --root /tmp/wn
   ```

2. Check the account through one discovery relay. Use an empty `--home` each time, because `wn` caches what it finds:

   ```sh
   RUST_MIN_STACK=8388608 /tmp/wn/bin/wn --home "$(mktemp -d)" --secret-store file --relay wss://nos.lol \
     --json keys check <npub or hex>
   ```

`--secret-store file` keeps `wn` out of the keychain. `--relay` is a hidden option. Without it, `wn` asks no
discovery relay unless its daemon is running.

## Contacts and invitations

Two people become contacts through a direct chat (#19). The interface's side is in
[core-interface.md](core-interface.md#contacts-and-invitations).

**The invite link** is `nostr:nprofile1…` (NIP-21): the public key and up to three of our relays as hints. The QR
code encodes the same string. On the Profile screen it is a version 9 QR code, 53 modules across.

What White Noise does, read from its current iOS and Android apps:

- **Its own links** are `marmot://profile/<npub>`. They carry no relay hints, and the Marmot spec defines no link
  format.
- **Its scanners** on both phones also read `nprofile1…` and `nostr:nprofile1…`, so they read our QR code. They drop
  the hints.
- **Pasting** an `nprofile` into its Android app's New chat field is refused; scanning works.
- **`marmot://profile/nprofile1…`** is refused by its Android app, so we don't use that form.
- **Neither app registers `nostr:`** as a link scheme; both register `marmot:` and `whitenoise:`.

We read White Noise's links in turn.

The check with White Noise itself (#22, [record](tests/22-white-noise.md)) passed with one gap:

- Starting a chat with White Noise works, and so does text both ways.
- White Noise 2026.11.17 can't start a chat with our accounts. It looks for relay lists only on its own two relays,
  where our accounts don't publish them. Whether they should is #76.

**Starting a chat.** `start_direct_conversation` does the work:

- It looks up the other person's relay lists and KeyPackage on our relays, then on the relays their list names.
  Relay lists that the invite link's hints lead to are fetched first and stored, and the lookup uses them too. The
  lookup gives up after 50 seconds.
- It creates an unnamed group of the two accounts, both admins, on our relays.
- It sends the MLS Welcome as a gift wrap (kind 1059) to the person's inbox relays (kind 10050), in the background
  after it returns.
- It returns the existing direct chat with that person instead, if there is one, including an invitation not yet
  accepted. Two people who start chats with each other at the same moment can still end up with two.

**Receiving.**

- MDK's running runtime follows the account's inbox relays, and it also picks up Welcomes that arrived while the app
  was closed.
- A Welcome becomes a group with `pending_confirmation` set and the inviter in `welcomer_account_id_hex`. The core
  shows that as an invitation.
- `subscribe_chats` delivers it without polling: a snapshot, then each changed group.

**Accepting** (`accept_group_invite`) only clears that flag on this device; the inviter is told nothing.

**Names.** MDK fetches no profile for the other person by itself. The core fetches it once, with a timeout, and MDK
keeps it, so it is there after a restart. Without one, the name is `default_profile_pseudonym`, the same pseudonym
White Noise shows.

**Not done yet:**

- **Declining or blocking an invitation.** `decline_group_invite` leaves the group, which can't be undone.
- **Groups.** The core ignores groups that aren't direct chats, for example a named group a White Noise user creates.
- **Late names.** A name learned after a chat appears reaches the interface only after a restart.
- **Opening a tapped link.** The app doesn't register a link scheme, so a link has to be pasted into New chat.

## Messages

Text in a direct chat (#20). The interface's side is in [core-interface.md](core-interface.md#messages).

**On the wire.**

- A message is a kind-9 event signed by the account. It is encrypted as an MLS application message and published to
  the chat's relays as a kind-445 event.
- Its id is the id of that inner kind-9 event. Both phones store the message under it, from sending through every
  restart, and it is the message's `id` in the interface.
- The id covers the text and the time in whole seconds, so the same text twice within a second would be one message.
  MDK refuses the second; the core sends it again a second later.

**Sending** uses `submit_text`, not `send_message`.

- `submit_text` stores the message and returns its id before any network work. The account's worker then publishes it.
- `send_message` waits up to 20 seconds for a relay and returns the id only afterwards.
- Unreachable relays are tried again every 5 seconds. When it is unclear whether a relay took a message, MDK backs
  off from 30 seconds to an hour.
- MDK resends the same signed bytes, so a message never doubles, and its queue survives a restart.
- `notify_connectivity_restored` cuts the backoff short. The core calls it when a relay comes back after an outage
  ([Relay reachability](#relay-reachability)).

**Delivery.** MDK's timeline record has no delivery field. The core reads it as MDK's chat list does:

- `sent` once the record has the published event's id (`source_message_id_hex`), or the recipient confirmed it;
- `waiting` before that.

A message that every relay refused is marked `local_publish_failed`; it stays `waiting` in the interface for now.

**Following.**

- `subscribe_timeline_messages` with no group follows every chat of the account. A received message, the user's own
  message when it is stored, and the same message once it is published each arrive as an upsert of its record.
- The core keeps kind-9 text. It drops system rows, media, stickers, deleted messages, and messages withdrawn while
  the chat's state was settled.
- It subscribes before it reads the chats' histories, so a message that arrives in between is never missed.

**History** comes from `timeline_messages_with_query`, at most 200 messages per chat: MDK's largest page. With a chat
given, MDK orders by MLS epoch, then by time and id. Its times are whole seconds.

**Messages before accepting.** MDK stores and shows messages that arrive in a chat whose invitation is still pending,
so the core adds them to the invitation.

## Profiles

**Publishing** (#93). `publish_user_profile_using_account_relays_and_indexers` publishes the account's profile (kind
0) to the account's relays. It also copies it to the relays it is given (#113).

- MDK merges what it is given with the newest profile it knows. Name, display name, about, picture, NIP-05 and
  lightning address are replaced, even when they are empty. Banner and other fields are kept unless the update sets them.
- So `update_profile` starts from the profile as this device has it (`directory_entry_for_account_id`), and changes only
  the name, display name and about. A photo or a lightning address set by another app stays.
- MDK publishes at once or fails at once. With no relay connected it fails within milliseconds; nothing is queued.
- **Copies on White Noise's relays** (#113). White Noise reads its users' contacts' profiles from its own relays. It
  keeps a copy there of what it found when it first looked the person up. Without a fresh copy there, White Noise users
  keep seeing the old name and photo. On 23 November, copying a new profile there by hand fixed that.
  - So an app on the default relays copies each profile change to `WHITE_NOISE_RELAYS`.
  - MDK sends the copies in the background once an account relay has the profile, so a failed copy never fails the
    change.
  - MDK never copies from an account whose relays include a loopback address. A build with relays of its own copies
    nothing (`profile_copy_relays`).

**Other people's profiles.** MDK fetches them when asked (`fetch_current_user_profile_for_account_id`), keeps what it
finds, and has no event for changes. The core fetches its contacts' profiles when it starts following the account and
every 5 minutes after, from our relays.

**Photos** (#94).

- **Uploading:** `upload_profile_image` puts a photo on a Blossom media server (BUD-02: `PUT /upload`, signed with the
  account key) and returns its address.
  - The core passes the server: `DEFAULT_MEDIA_SERVER`, blossom.primal.net, unless `MessagingOptions::media_server`
    names another.
  - MDK checks only the format and the size (10 MiB). It uploads the bytes as they are, metadata included. So the core
    saves the photo again first (`core/src/photo.rs`).
- **Downloading:** `download_profile_image` fetches a contact's photo. It takes public HTTPS addresses only, resolved and
  pinned, with limits on redirects and size.
  - With `MessagingOptions::allow_loopback`, a media server on this machine is fetched directly instead. This is for
    the tests and local development.
  - The same option lets MDK upload to such a server (`with_allow_loopback_blob_endpoints`), and connect to relays on
    this machine. It was called `allow_loopback_relays` before.

## App events

**Personal invitations' tokens** (#95) and **radio address records** (#34) travel as app events: `send_custom_event`
sends an event of a kind the app chooses, inside the chat's encryption.

- MDK refuses the kinds it acts on itself (chat, reactions, edits, deletions, polls, agent, group system, push tokens,
  device groups). The core uses kind 9910 (`INVITE_TOKEN_KIND`) for tokens and 9911 (`RADIO_ADDRESS_KIND`) for
  records. Both are provisional: Marmot assigns kinds only to adopted specs.
- The event shows in the timeline as a row of its own. It doesn't change the chat list's preview and raises no
  notification.
- It reaches the other phone while the chat is still a request. MDK delivers messages of chats not yet accepted, as it
  does text.

## Relay reachability

Whether a chat's messages can go out now (#43). The interface's side is in [core-interface.md](core-interface.md#reach).

**What MDK reports.** `subscribe_relay_liveness` gives the account's connection to each of its relays: connected,
connecting, offline or not tried yet.

- It covers the relays the account was set up with and the ones its relay lists name.
- MDK sees a connection close at once, and tries it again every 5 seconds. It pins nostr-sdk's retry interval, which
  would otherwise grow to a minute.
- A connection that goes silent without closing, as when a phone drops off Wi-Fi without the system closing its
  sockets, is noticed only when nostr-sdk's ping goes unanswered: after 55 to 110 seconds.

**What the core makes of it** (`core/src/reach.rs`):

- A relay counts as gone once its connection has stayed closed for 10 seconds (`DOWN_AFTER`), and as back as soon as
  it connects. Connecting counts as closed. A drop that MDK's next attempt mends doesn't show.
- A chat is `internet` while any of its relays is up, and `unreachable` when none is. `radio` comes with #44.
- A chat goes by the relays its group record names (`nostr_routing.relays`), where they are among the account's.
  Otherwise it goes by all of the account's relays, since MDK reports only those. That is the case for a chat that
  someone else's app set up on relays of its own.
- When a relay comes back after it counted as gone, the core calls `notify_connectivity_restored`. Waiting messages
  then go out at once rather than at their next retry.
- The phone's online flag plays no part: a phone can be online while no relay answers, behind a captive portal for
  example.
- Each relay's own status goes to the interface too (#88): connected; connecting while it opens or during a drop
  shorter than `DOWN_AFTER`; unreachable after that. Who runs a relay comes from `PROJECT_RELAYS`
  (`core/src/messaging.rs`), which stays empty until the project runs one (#11).

**Timing.** Blocking the relays makes a chat `unreachable` 10 seconds after MDK sees the connections close. Unblocking
makes it `internet` again with MDK's next attempt, within 5 seconds.

**After an outage.** The sender's waiting messages go out within a second of the relays coming back. The other phone
sometimes shows them much later. In the tests it got them after 1 to 20 seconds in most runs, but in some not within 70
seconds, and in one after 7.6 minutes.

- On reconnecting, MDK asks the relay again for the chat's events. Events in that reply that it hasn't seen are held
  until its history recovery proves that nothing older is missing (the replay fence, see [Tests](#tests)).
- That proof needs the relay's NIP-11 limits. MDK caches a failed fetch of them as unknown for 5 minutes
  (`UNKNOWN_FRESH_FOR` in `marmot-app/src/relay_caps.rs`), and a fetch tried during the outage fails. With the test
  relay still answering NIP-11 requests while blocked, the message arrived within 5 seconds every time.
- Whether it shows depends on timing: on whether the sender publishes before the other phone has asked again.
- MDK has no public call that clears the cache. What to do about it is #78.
- **Since #46,** a message written during an outage waits in the core and goes only once a relay is connected again,
  so the other phone has usually asked again by then. `core/tests/after_outage.rs` (ignored; it can take minutes)
  measured 5.1 seconds from the relay's return to the other phone showing the message, in 4 of 4 runs with the relay
  fully blocked. Before #46, with MDK holding the message, the same runs took 25 seconds. MDK's full-history repair
  (`repair_full_history`) on the receiving phone made no difference. Still exposed: messages MDK took in the 10
  seconds before the outage counted, and the relay copies of radio messages, which the other phone shows already.
- Tests about something else can keep this out: the test relay's `block_messages` goes on answering NIP-11 requests
  while it blocks everything else (`core/tests/republish.rs`).

## Tests

`core/tests/relay_round_trip.rs` creates an account against a relay running inside the test
(`nostr_relay_builder::MockRelay`, as MDK's own tests use) and reads the published events back with a separate
`nostr-sdk` client. Keys go to `keyring-core`'s in-memory store, so the test never touches the machine's keychain and
runs on CI without a Secret Service. In a second test, another MDK instance that knows only the relay finds the
account's relay lists and fetches its KeyPackage, which is what it needs to invite the account.

`core/tests/core_interface.rs` does the same for `Core`:

- It creates an account, finds it again after a restart and deletes it.
- It checks the interface's JSON against the shared fixture.
- It checks that the account key stays in the key store:
  - the key appears in no command result, event or snapshot;
  - no file in the data folder holds it;
  - deleting the account removes it.

`core/tests/contacts.rs` runs two `Core`s, as two phones, against one relay (#19):

- one adds the other from their invite link, and the chat opens with the other's name;
- the other receives the invitation with the inviter's name, accepts it, and gets the same chat;
- adding again returns that chat, and both keep it, with names, across a restart;
- the commands' error messages.

The link parsing has unit tests in `core/src/invite.rs`, including White Noise's link shapes.

`core/tests/messages.rs` has two contacts exchange text (#20):

- each message arrives under the same id and time;
- delivery moves from `waiting` to `sent`;
- both histories survive a restart;
- the same text twice within a second makes two messages;
- a message sent before the invitation is accepted comes with it;
- the error messages.

`core/tests/reachability.rs` blocks and unblocks the two phones' relay (#43):

- a one-second drop changes nothing;
- blocked, the chat becomes `unreachable` on both phones 10 to 15 seconds later, and so does the relay in the
  snapshot. A message written then waits;
- unblocked, the chat and the relay are back within 10 seconds, and the waiting message goes out. The test doesn't wait
  for it to arrive, which can take minutes ([After an outage](#relay-reachability));
- an app that starts without its relay marks its chats `unreachable` after 10 seconds.

The tracker itself has unit tests in `core/src/reach.rs`.

`core/tests/invites.rs` has three phones share one personal invitation (#95):

- the invited person, without an account, sees who invited her, then creates an account and starts the chat;
- the inviter's core opens the chat without a request and marks the invitation as used;
- a third person using the same link arrives as an ordinary request;
- the invitation stays across a restart, and deleting the account deletes it.

The invitation store has unit tests in `core/src/invites.rs`.

`core/tests/photos.rs` has a contact set a photo straight from a camera (#94), through a media server inside the test
(`core/tests/support/blossom.rs`):

- what reaches the server is a 512-pixel square without the Exif segment;
- the other phone shows it once its core fetches the profiles again, and both keep it across a restart;
- removing it reaches the other phone; a file that isn't a photo is refused before any upload;
- deleting the account deletes the avatars.

The photo handling itself has unit tests in `core/src/photo.rs`: the square, turning a photo upright by its Exif
orientation, dropping the metadata, transparency and refusals.

`core/tests/profile.rs` has a contact change her profile (#93):

- the new name and about come back as `accountChanged`, and stay across a restart;
- the other phone shows the new name once its core fetches the profiles again, and a second fetch finds nothing new;
- without a relay the profile doesn't change, and the error says why;
- the error messages, and the limits counted in characters.

`core/tests/radio_keys.rs` has two contacts derive their chat's radio keys (#37):

- what one sends with, the other receives with, and a restart keeps the keys;
- a slow test, ignored unless asked for (`--include-ignored`), checks that both move to new keys after MDK's
  post-join key update and after one asked for. Each took about 100 seconds.

The two-phone tests share `core/tests/support/mod.rs`, and take turns: several MDK runtimes in one process slow each
other down. Three tests took 31 seconds together and 11 one after another. The app runs one runtime.

Their relay (`core/tests/support/relay.rs`) describes itself the way public relays do. A relay from
`nostr_relay_builder` serves no NIP-11 information document. Without one, MDK can't tell whether it has seen a group's
whole history, and holds back the group's messages stored before the member joined, for about 15 minutes. So the
tests reach the relay through a front door that answers NIP-11 requests with the filter limit the relay enforces, as
MDK's own tests do. The door can be shut: it then cuts every connection and refuses new ones.
`core/examples/local_relay.rs` runs the same relay on the Mac for apps on phones, and `block` and `unblock` shut and
open its door.

## Phones

The core builds and runs on iOS and Android. Sizes, build times and caveats are in
[spikes/15-mdk-on-phones.md](spikes/15-mdk-on-phones.md).

## Pin

| What | Where |
| --- | --- |
| MDK release tag (`v0.18.0`) | `core/Cargo.toml` |
| MDK commit (`34646a577807f2b112ee031f3c164ec58aa18b64`) | `Cargo.lock`, entry `marmot-app` |
| Patches MDK's workspace applies | `[patch.crates-io]` in `Cargo.toml` |
| Odin compiler version | MDK's `scripts/install-odin.sh` at the locked commit |

Always pin a release tag: MDK deletes nightly builds after 30 days, and the `mdk-core` crate on crates.io is stale.

## Build requirements

- **Odin.** MDK's Markdown parser (`marmot-markdown`) compiles Odin code at build time. `scripts/install-odin.sh`
  installs the compiler version the locked MDK pins into `.tools/odin`, where `.cargo/config.toml` points `ODIN`. Every
  build of the core or the app needs it, CI included.
- **LLVM tools.** For Android builds and optimized Linux builds, the Markdown parser compiles through the Rust
  toolchain's `llvm-tools-preview` component, which `rust-toolchain.toml` lists.
- **Android API level.** The Markdown parser builds for Android API 26 unless `ANDROID_API` says otherwise.
  `.cargo/config.toml` sets it to 24, the app's `minSdk`.
- **`ranlib` on Android.** OpenSSL's build calls `<target>-ranlib`, which current NDKs don't ship. The README's Android
  setup points `RANLIB_<target>` at the NDK's `llvm-ranlib`.
- **SQLCipher and OpenSSL** are compiled from source (vendored). A C compiler, `perl` and `make` are enough; no system
  libraries are needed.
- **Stack size.** Unoptimized OpenMLS can overflow the default 2 MiB thread stack.
  - `.cargo/config.toml` sets `RUST_MIN_STACK` to 4 MiB, as MDK does. That reaches only processes Cargo starts, such as
    tests and desktop dev builds.
  - The app shell builds Tauri's async runtime with 8 MiB thread stacks itself, so phones get them too.
- **Release profile.** Panics must unwind: MDK isolates failures per account worker, and `panic = "abort"` would end the
  whole app instead. The workspace uses MDK's recommended thin LTO.

## Patches and locked versions

Cargo applies `[patch]` sections only from the top-level workspace, so MDK's own patches do not reach us. We copy them
into `Cargo.toml`. A patch only applies when the patched version fits the dependency graph, so some crates must stay at
the version MDK's `Cargo.lock` has. Today that is `rusqlite` 0.40.1, which lets the patched `libsqlite3-sys` 0.38.1
(SQLCipher 4.17.0) replace the crates.io one.

Cargo prints `patch ... was not used in the crate graph` when a patch does not apply. Treat that warning as an error.

## Upgrading MDK

1. Read MDK's integration guide for every release in between (`docs/integration/<version>.md`) and the release notes
   (`docs/release/<version>.md`), at the new tag.
2. Change the tag in `core/Cargo.toml` and run `cargo update -p marmot-app`.
3. Compare MDK's `[patch.crates-io]` and `[profile.*]` sections with ours and copy the changes.
4. Run `cargo fetch`. If it warns that a patch was not used, move the crate that pulls it in to the version in MDK's
   `Cargo.lock`: `cargo update -p <crate> --precise <version>`.
5. Run `scripts/install-odin.sh`; the new release may pin another Odin.
6. Run the checks in the README, including the relay round trip in `cargo test -p peoples-chat-core`.
7. Update the pin table above.
