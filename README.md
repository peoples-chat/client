# peoples.chat

An everyday encrypted messenger that keeps working over LoRa radio when the internet is gone, with one
identity and one contact list for both.

**Status:** proof of concept. The app creates an account, adds contacts by QR code or invite link, and chats over
Nostr relays. The core notices when the relays can't be reached. The radio is still to come.
Nothing here is ready for real use. No licence has been chosen yet, so all rights are reserved for now.

Work is tracked in the issues of this repository.

## Layout

| Path | What it is |
| --- | --- |
| `core/` | Rust core (`peoples-chat-core`). All protocol logic goes here. It must not depend on Tauri or on interface code. It builds on MDK; see [docs/mdk.md](docs/mdk.md). |
| `fake-radio/` | A fake MeshCore companion radio over TCP, for tests without hardware ([docs/radio.md](docs/radio.md#fake-radios-over-tcp)). |
| `src-tauri/` | Tauri 2 app shell (`peoples-chat`). Hosts the web view and forwards commands to the core. |
| `src-tauri/plugins/share-sheet/` | A Tauri plugin of the app's own: the system share sheet on iOS (Swift) and Android (Kotlin), which the phones' web views can't open. |
| `src-tauri/gen/apple`, `src-tauri/gen/android` | Generated Xcode and Android Studio projects. They are committed. |
| `src-tauri/icons/source/` | The logo as app icon sources. `pnpm tauri icon src-tauri/icons/source/manifest.json` regenerates every icon, including those in the generated projects. |
| `src/` | Web interface (React, TypeScript, Vite). It holds no protocol logic. |
| `src/core/` | The interface's side of the boundary to the core: commands, events, the bridge to the real core and a mock core with demo data. |
| `docs/` | The spec ([docs/spec.md](docs/spec.md)), the interface between core and web view ([docs/core-interface.md](docs/core-interface.md)), MDK in the core ([docs/mdk.md](docs/mdk.md)), radio transports ([docs/radio.md](docs/radio.md)), the radio message format ([docs/radio-format-v0.md](docs/radio-format-v0.md)) and findings from spikes (`docs/spikes/`). |
| `relay/` | The relay the project runs (#90): its configuration, write policy and setup script, and how to check a relay ([relay/README.md](relay/README.md)). |
| `.github/workflows/ci.yml` | CI: format, clippy and tests for the core, type check, tests and build check for the app. |

Keep the interface thin: it shows state and sends commands, and the core does the rest.

## Prerequisites

- **Rust** through [rustup](https://rustup.rs). The toolchain is pinned in `rust-toolchain.toml` and is
  installed automatically on first use.
- **Node.js** 24 or newer and **pnpm** 12.
- The [Tauri system dependencies](https://tauri.app/start/prerequisites/) for your platform.
- **Odin**, which MDK's Markdown parser needs at build time: run `scripts/install-odin.sh` once, and again after an MDK
  upgrade. It installs the pinned compiler into `.tools/odin`. It needs `python3` and `curl`.

For iOS (macOS only):

- Xcode with an iOS simulator. If `pnpm tauri ios dev` says the simulator SDK is not installed and opens
  Xcode, let Xcode finish starting once and run the command again.
- `brew install xcodegen cocoapods`
- `rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios`

For Android:

- Android SDK with platform tools, an emulator image and the NDK (tested with NDK 29.0.14206865).
- A JDK that the Android Gradle plugin supports (tested with JDK 21).
- `rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android`
- These environment variables:

  ```sh
  export JAVA_HOME=/path/to/jdk-21
  export ANDROID_HOME="$HOME/Library/Android/sdk"
  export NDK_HOME="$ANDROID_HOME/ndk/29.0.14206865"
  # MDK builds OpenSSL from source, which needs the NDK's llvm-ranlib; Tauri sets only the
  # compiler and archiver. (On Linux, the folder is linux-x86_64.)
  for target in aarch64_linux_android armv7_linux_androideabi i686_linux_android x86_64_linux_android; do
    export "RANLIB_$target=$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-ranlib"
  done
  ```

## Run

```sh
pnpm install

pnpm tauri dev                       # desktop
pnpm tauri ios dev "iPhone 17"       # iOS simulator
pnpm tauri android dev               # Android emulator (start the emulator first)
```

Settings shows `Core 0.1.0` at the bottom when the web view, the app shell and the Rust core are wired
together.

`pnpm dev` alone serves the interface at http://localhost:1420 for a browser, on the mock core. That is the
quickest way to work on screens. Anything the app cannot do yet is disabled.

The app uses the real core. It publishes new accounts to public relays. Development builds on a computer keep the
account key in a file in the app's data folder, so the Mac's keychain doesn't ask for your password at every start;
build with `TAURI_PEOPLES_CHAT_KEYCHAIN=1` to use the keychain instead. To see the demo data in the app instead, start
it with `VITE_CORE=mock`, for example
`VITE_CORE=mock pnpm tauri dev`. Relays, keys and the rest are in
[docs/core-interface.md](docs/core-interface.md#running-the-app-with-the-real-core).

The app's product name is `peopleschat`, without the dot, because `pnpm tauri ios build` cannot find the built app
when the name has one. People still see "peoples.chat": on iOS through `CFBundleDisplayName` in
`src-tauri/Info.ios.plist`, on Android through `app_name` in the generated project.

## Checks

The same checks run in CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p peoples-chat-core      # includes a relay round trip through MDK
pnpm check
pnpm test
pnpm build
```

## Releases

`pnpm release:android` and `pnpm release:ios` build unsigned release artefacts into `target/release-artifacts/`, and
`pnpm version:check` checks that every file carries the same version. Version numbers, signing and the proposed release
workflow are in [docs/release/README.md](docs/release/README.md).

## Radio lab

With the radio lab's hardware plugged in, `pnpm verify:radio` runs the checks that need it
([docs/radio-lab.md](docs/radio-lab.md)).
