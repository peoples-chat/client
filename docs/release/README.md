# Releases for Android and iOS

How peoples.chat builds release artefacts for the phones (#144). It follows
[eddiechat/client](https://github.com/eddiechat/client) (`build.yml` and the release skill at commit `69ff084`),
cut down to Android and iOS.

| File | What it is |
| --- | --- |
| [`build.yml`](build.yml) | The proposed release workflow. It belongs in `.github/workflows/release.yml`, which agents may not edit, so it waits here until it is approved and installed. |
| [`SKILL.md`](SKILL.md) | The release skill for Claude Code. It belongs in `.claude/skills/release/SKILL.md`, for the same reason. |
| [`../../scripts/release/version.mjs`](../../scripts/release/version.mjs) | Reads, checks and sets the version in every file that carries it. |
| [`../../scripts/release/android.sh`](../../scripts/release/android.sh) | Builds an unsigned release APK and AAB. |
| [`../../scripts/release/ios.sh`](../../scripts/release/ios.sh) | Builds an iOS release: an IPA when signing is set up, otherwise an unsigned archive. |

## Version numbers

One version, `MAJOR.MINOR.PATCH`, for the whole app. Releases are tagged `vMAJOR.MINOR.PATCH`. The version is written in
these files, and `node scripts/release/version.mjs` fails when they disagree:

- `package.json`, `src-tauri/tauri.conf.json` and `[workspace.package]` in `Cargo.toml` (with our crates in `Cargo.lock`).
  Settings shows the core's crate version, so it is the app's version too.
- `CFBundleShortVersionString` and `CFBundleVersion` in `src-tauri/gen/apple/project.yml` and
  `src-tauri/gen/apple/peoples-chat_iOS/Info.plist`. `pnpm tauri ios build` rewrites them from `tauri.conf.json` anyway;
  the script keeps the committed copies from drifting.

`node scripts/release/version.mjs set 0.2.0` writes a new version into all of them; `from-tag v0.2.0` does the same from a tag,
which is what the workflow does, so a tag and the app it builds always agree.

The build numbers the stores need come from the version and the CI run:

- **Android `versionCode`**: Tauri derives it as `MAJOR × 1 000 000 + MINOR × 1000 + PATCH` (0.1.0 is 1000). Each
  version can therefore go to Google Play once; minor and patch must stay below 1000.
- **iOS `CFBundleVersion`**: the version, with the workflow's run number appended (`0.2.0.17`), so a version can go to
  TestFlight more than once.

## Building on a Mac

The tools are those in the README's Prerequisites.

```sh
pnpm release:android     # target/release-artifacts/peoples-chat-<version>-android-unsigned.{apk,aab}
pnpm release:ios         # target/release-artifacts/peoples-chat-<version>-ios-unsigned.xcarchive.zip
```

An unsigned APK does not install on a phone. To try one, sign it with a throwaway key; a debug build
(`pnpm tauri android build --debug`) is simpler for a quick look.

For iOS, `pnpm release:ios` signs and exports an IPA (`peoples-chat-<version>-ios.ipa`) when `APPLE_DEVELOPMENT_TEAM` is
set and the Mac has a distribution certificate and provisioning profile for `chat.peoples.client`. Without them it builds
with `--no-sign` and stops at the archive: the app is compiled and bundled, but an IPA needs a signature (#61).

## Signing and the secrets the workflow reads

No key, certificate or profile is in this repository. The workflow reads them from these GitHub Actions secrets; any
that are missing make it fall back to unsigned artefacts and skip the upload.

| Secret | What it holds |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | The Play upload keystore (`.jks`), base64-encoded. |
| `ANDROID_KEYSTORE_PASSWORD` | Its store password. |
| `ANDROID_KEY_ALIAS` | The alias of the upload key in it. |
| `ANDROID_KEY_PASSWORD` | The key's password. |
| `IOS_CERTIFICATE` | The Apple Distribution certificate with its private key (`.p12`), base64-encoded. |
| `IOS_CERTIFICATE_PASSWORD` | The `.p12`'s password. |
| `IOS_MOBILE_PROVISION` | The App Store provisioning profile for `chat.peoples.client`, base64-encoded. |
| `APPLE_DEVELOPMENT_TEAM` | The Apple team ID. |
| `APPLE_API_KEY_ID` | The App Store Connect API key's ID, for the TestFlight upload. |
| `APPLE_API_ISSUER` | Its issuer ID. |
| `APPLE_API_KEY` | The key itself (the `.p8` file's text). |

Android artefacts are signed after the build, with `apksigner` (APK) and `jarsigner` (AAB), so the Gradle project has no
signing configuration and no key file passes through it. For iOS, `pnpm tauri ios build` imports `IOS_CERTIFICATE` and
`IOS_MOBILE_PROVISION` into a temporary keychain itself.

## Installing the workflow and the skill

Once approved:

```sh
cp docs/release/build.yml .github/workflows/release.yml
mkdir -p .claude/skills/release && cp docs/release/SKILL.md .claude/skills/release/SKILL.md
```

and delete the two copies here, then add the secrets above in the repository's settings.

## What the workflow does

On a `v*` tag, or by hand from the Actions tab:

1. **Android** (Ubuntu): sets the version from the tag, installs JDK 21, the SDK and NDK 29.0.14206865, runs
   `scripts/release/android.sh` and signs the results if the Android secrets are there.
2. **iOS** (macOS): sets the version, runs `scripts/release/ios.sh` with the run number as build number, and uploads the IPA
   to TestFlight if it is signed and the App Store Connect key is there.
3. **GitHub release** (tags only): a draft release with the APK, AAB and IPA, its text from `RELEASE_NOTES.md`, which
   the release skill writes. A person publishes it.

Unlike eddiechat, it builds no desktop apps and doesn't re-create the generated Android and Xcode projects: ours are
committed and carry our own changes.
