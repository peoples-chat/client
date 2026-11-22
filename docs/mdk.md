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
| `close` | `shutdown_and_close` |

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
  don't need to be reachable.
- **Deleting wipes.** `sign_out_and_wipe` removes the account's databases and its key. Telling relays and groups is
  best effort and never blocks the local wipe.

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

## Tests

`core/tests/relay_round_trip.rs` creates an account against a relay running inside the test
(`nostr_relay_builder::MockRelay`, as MDK's own tests use) and reads the published events back with a separate
`nostr-sdk` client. Keys go to `keyring-core`'s in-memory store, so the test never touches the machine's keychain and
runs on CI without a Secret Service.

`core/tests/core_interface.rs` does the same for `Core`:

- It creates an account, finds it again after a restart and deletes it.
- It checks the interface's JSON against the shared fixture.
- It checks that the account key stays in the key store:
  - the key appears in no command result, event or snapshot;
  - no file in the data folder holds it;
  - deleting the account removes it.

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
