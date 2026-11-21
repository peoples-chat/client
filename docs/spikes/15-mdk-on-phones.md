# Spike #15: the MDK-linked core on iOS and Android

**Result: works, with caveats.** Tested 21 November 2026 with MDK v0.18.0.

The app builds for every iOS and Android target with MDK linked into the core, and it runs on both platforms. In the
iOS simulator and the Android emulator it created an account and published the account's relay lists, profile and
KeyPackage to a relay. The relay acknowledged each one.

MDK adds about 5 MB to the iOS app and 6.5 MB to the Android APK. A first release build takes about four minutes per
target, compared with about one minute without MDK.

The caveats concern the build setup and what simulators can't show; see [Caveats](#caveats).

## Relay round trip

A command that exists only in debug builds (`src-tauri/src/spike.rs`) runs the test:

1. It opens the core's `Messaging` in a fresh folder.
2. It creates an account named "Spike".
3. It waits until the relay has acknowledged every event the account publishes.

The relay runs on the Mac (`core/examples/local_relay.rs`) and prints each event it stores.

| | iOS simulator | Android emulator |
| --- | --- | --- |
| Device | iPhone 17, iOS 26.5 | Pixel 10 Pro, Android 17 (API 37), arm64 |
| Build | `pnpm tauri ios build --debug --target aarch64-sim` | `pnpm tauri android build --debug --apk --target aarch64` |
| Open messaging | 127 ms | 51 ms |
| Create account | 2,043 ms | 1,653 ms |
| Publish until acknowledged | 104 ms | 105 ms |
| Close | 18 ms | 14 ms |

The relay received five event kinds from both accounts: 10002 and 10050 (relay lists), 3 (follow list), 0 (profile) and
30443 (KeyPackage).

MDK stored the keys through its keyring stores: `apple-native-keyring-store` on iOS and
`android-native-keyring-store` on Android.

The times come from debug builds, where unoptimized OpenMLS slows down account creation. An earlier Android run gave
the same picture; its Markdown parser was built for API 26.

## Builds

Each time is the first build of that target and profile. The crates were already downloaded and the host build tools
cached.

Every target compiles SQLCipher, OpenSSL and MDK's Odin-built Markdown parser from source.

| Target | Command | Result | Time |
| --- | --- | --- | --- |
| iOS simulator arm64, debug | `pnpm tauri ios build --debug --target aarch64-sim` | App builds and runs; the command fails at its last step ([below](#build-setup)) | 69 s |
| iOS simulator arm64, release | `pnpm tauri ios build --target aarch64-sim` | App builds; the same last step fails | 246 s |
| iOS simulator x86-64, debug | `cargo build -p peoples-chat --lib --target x86_64-apple-ios` | Builds | 66 s |
| iOS device, debug | `cargo build -p peoples-chat --lib --target aarch64-apple-ios` | Builds; not linked into an app ([below](#not-tested-yet)) | 68 s |
| Android arm64, debug | `pnpm tauri android build --debug --apk --target aarch64` | APK builds and runs | 57 s |
| Android, all four ABIs, debug | `pnpm tauri android build --debug --apk` | APK builds | 226 s |
| Android arm64, release | `pnpm tauri android build --apk --target aarch64` | APK builds | 268 s |

The build machine:

- Apple M5 Max (18 cores, 64 GB), macOS 27.2.1
- Xcode 27.2, Android NDK 29.0.14206865, Rust 1.97.1

For comparison, we built the app without MDK: main just before MDK was linked (f3d1bb3), with today's release profile,
from an empty target folder.

- Android arm64 release build: 65 s.
- iOS simulator release build: 41 s.

Builds with nothing for Rust to recompile are quick:

- iOS simulator debug: 16 s.
- Android arm64 debug: 14 s.

## Size

These are release builds for arm64 only. "Without MDK" is the same comparison build (f3d1bb3, same release profile).

| | Without MDK | With MDK | MDK adds |
| --- | --- | --- | --- |
| Android APK, unsigned | 7.5 MB | 14.0 MB | 6.5 MB |
| of which the app's native library | 5.4 MB | 11.9 MB | 6.5 MB |
| iOS app, simulator | 4.2 MB | 9.4 MB | 5.1 MB |
| of which the executable | 4.1 MB | 9.3 MB | 5.1 MB |

App store download sizes will differ; these numbers compare like with like.

## Caveats

### Build setup

- **Android builds need `RANLIB_<target>`.**
  - OpenSSL's build looks for `<target>-ranlib`. NDK 29 doesn't have it, and Tauri sets only the compiler and archiver.
  - The README's Android setup exports the NDK's `llvm-ranlib` for each ABI. A CI job for Android needs the same.
  - The path depends on where the NDK is installed, so it can't go in `.cargo/config.toml`.
- **`pnpm tauri ios build` fails at its last step.**
  - Xcode builds and archives the app. The Tauri CLI then looks for `peoples.app` to rename it, but the archive holds
    `peoples.chat.app`. The CLI seems to treat `.chat` in the product name as a file extension.
  - The same happens without MDK.
  - The archived app is complete and installs in the simulator, and `pnpm tauri ios dev` is not affected.
  - It has to be solved before the build can produce an IPA for devices: a product name without a dot, or a fix in
    Tauri.

### Not tested yet

- **iOS devices.**
  - The Rust library compiles for `aarch64-apple-ios`, but the device app needs a code-signing identity, and the build
    machine has none.
  - Linking, signing and running on an iPhone wait for #12. The simulator build links the same code for the same CPU,
    so we expect no surprises.
- **Real devices, mainly their key stores.** These include keychain access in a signed iPhone app and Android's
  hardware-backed keystore. The spike command stays in debug builds, so the same run can be repeated on devices.
- **Android 7.**
  - MDK's Markdown parser builds for Android API 26 unless told otherwise. `.cargo/config.toml` now sets `ANDROID_API`
    to 24, the app's `minSdk`.
  - That build ran on the API 37 emulator. Nothing has run on Android 7 (API 24 and 25).
- **TLS.**
  - The spike used `ws://` to a relay on the Mac. The iOS simulator shares the Mac's network, and the emulator reached
    the relay through `adb reverse`.
  - Real relays use `wss://`. The relay connection uses rustls with bundled root certificates (`webpki-roots`), so TLS
    should need no phone-specific setup. It hasn't run on a phone yet.

## What the spike changed

- **`src-tauri/src/spike.rs`**:
  - The debug-only command `spike_relay_round_trip`. It also runs at launch when `PEOPLES_CHAT_SPIKE_RELAY` holds a
    relay URL.
  - It can go once the real core commands (#17) cover the same ground.
- **`core/examples/local_relay.rs`**: a relay for the Mac that prints each event it stores.
- **`src-tauri/src/lib.rs`**:
  - Tauri's async runtime now has 8 MiB thread stacks. Unoptimized OpenMLS can outgrow the default 2 MiB, and MDK's
    runtime tests use 8 MiB.
  - `RUST_MIN_STACK` in `.cargo/config.toml` reaches only processes Cargo starts, such as tests and `pnpm tauri dev`. It
    doesn't reach an app that a phone starts.
  - The spike ran with 8 MiB stacks; the default was not tried.
- **`.cargo/config.toml`**: `ANDROID_API = "24"`.

## Run it again

Spike accounts use their own folder and key store service, separate from the app's own.

1. Start the relay:

   ```sh
   cargo run -p peoples-chat-core --example local_relay -- 7777
   ```

2. On a booted iOS simulator:

   ```sh
   pnpm tauri ios build --debug --target aarch64-sim   # ends with the rename error above
   xcrun simctl install booted src-tauri/gen/apple/build/peoples-chat_iOS.xcarchive/Products/Applications/peoples.chat.app
   SIMCTL_CHILD_PEOPLES_CHAT_SPIKE_RELAY=ws://127.0.0.1:7777 \
     xcrun simctl launch --terminate-running-process booted chat.peoples.client
   ```

   After a few seconds, read the outcome:

   ```sh
   find "$(xcrun simctl get_app_container booted chat.peoples.client data)" -path '*/spike/latest.json' -exec cat {} \;
   ```

3. On a running Android emulator, with the environment from the README:

   ```sh
   pnpm tauri android build --debug --apk --target aarch64
   adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
   adb reverse tcp:7777 tcp:7777
   ```

   Then:

   1. Start the app.
   2. Open `chrome://inspect` in Chrome and inspect the app's web view.
   3. Run this in the console:

      ```js
      await __TAURI_INTERNALS__.invoke("spike_relay_round_trip", { relay: "ws://127.0.0.1:7777" })
      ```

   The outcome is also stored in the app; read it with `adb shell run-as chat.peoples.client cat spike/latest.json`.
