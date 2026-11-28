#!/usr/bin/env bash
# Builds an unsigned Android release APK and AAB (#144) and copies them to target/release-artifacts/.
#   scripts/release/android.sh [extra `tauri android build` arguments]
# Signing happens afterwards, outside the build (docs/release/README.md), so no key ever reaches
# Gradle or this repository.
set -euo pipefail
cd "$(dirname "$0")/../.."

fail() { echo "android.sh: $*" >&2; exit 1; }

version="$(node scripts/release/version.mjs)"

: "${JAVA_HOME:?set JAVA_HOME to a JDK 21 (README, Prerequisites)}"
: "${ANDROID_HOME:=${ANDROID_SDK_ROOT:-}}"
[[ -n "$ANDROID_HOME" ]] || fail "set ANDROID_HOME to the Android SDK (README, Prerequisites)"
export ANDROID_HOME
: "${NDK_HOME:?set NDK_HOME to the NDK, for example \$ANDROID_HOME/ndk/29.0.14206865}"
[[ -d "$NDK_HOME" ]] || fail "NDK_HOME=$NDK_HOME does not exist"
[[ -x .tools/odin/odin ]] || scripts/install-odin.sh

# MDK builds OpenSSL from source, which needs the NDK's llvm-ranlib; Tauri sets only the compiler
# and archiver.
case "$(uname -s)" in
  Darwin) host=darwin-x86_64 ;;
  *) host=linux-x86_64 ;;
esac
for target in aarch64_linux_android armv7_linux_androideabi i686_linux_android x86_64_linux_android; do
  var="RANLIB_$target"
  [[ -n "${!var:-}" ]] || export "$var=$NDK_HOME/toolchains/llvm/prebuilt/$host/bin/llvm-ranlib"
done

pnpm tauri android build --ci --apk --aab "$@"

outputs=src-tauri/gen/android/app/build/outputs
apk="$outputs/apk/universal/release/app-universal-release-unsigned.apk"
aab="$outputs/bundle/universalRelease/app-universal-release.aab"
[[ -f "$apk" ]] || fail "no APK at $apk"
[[ -f "$aab" ]] || fail "no AAB at $aab"

out=target/release-artifacts
mkdir -p "$out"
cp "$apk" "$out/peoples-chat-$version-android-unsigned.apk"
cp "$aab" "$out/peoples-chat-$version-android-unsigned.aab"
echo
echo "Unsigned Android release $version:"
ls -l "$out"/peoples-chat-"$version"-android-unsigned.*
