# peoples.chat

An everyday encrypted messenger that keeps working over LoRa radio when the internet is gone, with one
identity and one contact list for both.

**Status:** proof of concept. The interface runs on a mock core with demo data until the real core lands.
Nothing here is ready for real use. No licence has been chosen yet, so all rights are reserved for now.

Work is tracked in the issues of this repository.

## Layout

| Path | What it is |
| --- | --- |
| `core/` | Rust core (`peoples-chat-core`). All protocol logic goes here. It must not depend on Tauri or on interface code. It builds on MDK; see [docs/mdk.md](docs/mdk.md). |
| `src-tauri/` | Tauri 2 app shell (`peoples-chat`). Hosts the web view and forwards commands to the core. |
| `src-tauri/gen/apple`, `src-tauri/gen/android` | Generated Xcode and Android Studio projects. They are committed. |
| `src-tauri/icons/source/` | The logo as app icon sources. `pnpm tauri icon src-tauri/icons/source/manifest.json` regenerates every icon, including those in the generated projects. |
| `src/` | Web interface (React, TypeScript, Vite). It holds no protocol logic. |
| `src/core/` | The interface's side of the boundary to the core: commands, events and, for now, a mock core with demo data. |
| `docs/` | Notes on MDK in the core ([docs/mdk.md](docs/mdk.md)) and findings from spikes (`docs/spikes/`). |
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

`pnpm tauri ios build` fails at its last step, after Xcode has built the app; see
[docs/spikes/15-mdk-on-phones.md](docs/spikes/15-mdk-on-phones.md#build-setup).

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
