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
| `Messaging::open` | `AccountHome::open_with_keychain`, `MarmotApp::try_with_relays_and_account_home_and_config`, `MarmotAppRuntime::start` |
| `create_account(name)` | `create_identity_local_ready_with_initial_profile` |
| `accounts()` | `AccountManager::managed_accounts`, `display_name_for_account_id` |
| `delete_account` | `sign_out_and_wipe` |
| `wait_until_published` | polls `account_setup_readiness` until `NetworkReady` |
| `start_direct_chat` | `Directory::fetch_account_relay_list_status_for_account_id` (relay hints), `start_direct_conversation` |
| `accept_direct_chat` | `accept_group_invite` |
| `follow_direct_chats` | `subscribe_chats` |
| `known_name`, `contact_name` | `display_name_for_account_id`, `fetch_current_user_profile_for_account_id`, `default_profile_pseudonym` |
| `chat_started_at` | `chat_list_row` |
| `send_text` | `submit_text` |
| `chat_messages`, `chat_message` | `timeline_messages_with_query`, `timeline_message` |
| `follow_messages` | `subscribe_timeline_messages` |
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
ours reads, holds or passes the key.

**Where it lives.** `AccountHome::open_with_keychain` keeps one entry per account in the platform's key store:

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

**What the web view sees:** the public key and the invite link, never the key.

**Not checked yet:** keychain access in a signed iPhone app, and Android's hardware-backed keystore on a real phone (#12).

## Relays

**Defaults.** New accounts use three public, general-purpose relays: `wss://relay.damus.io`, `wss://nos.lol` and
`wss://relay.primal.net` (`DEFAULT_RELAYS` in `core/src/messaging.rs`). Who runs the project's own relays is decided
later (#11). Building with `PEOPLES_CHAT_RELAYS` (comma-separated) replaces them; see
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

We read White Noise's links in turn. Checking with White Noise itself is #22.

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
- `notify_connectivity_restored` cuts the backoff short. The core doesn't call it yet; #43 watches relay reachability.

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

The two-phone tests share `core/tests/support/mod.rs`, and take turns: several MDK runtimes in one process slow each
other down. Three tests took 31 seconds together and 11 one after another. The app runs one runtime.

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
