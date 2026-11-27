#!/usr/bin/env bash
# Builds an iOS release (#144) into target/release-artifacts/.
#   scripts/release/ios.sh [extra `tauri ios build` arguments]
# With APPLE_DEVELOPMENT_TEAM set (and a distribution certificate and profile, in the keychain or
# in IOS_CERTIFICATE and IOS_MOBILE_PROVISION), it signs and exports an IPA for App Store Connect.
# Without it, it builds with --no-sign and stops at the archive: an IPA needs a signature (#61).
# BUILD_NUMBER, if set, is appended to the version as CFBundleVersion (0.2.0.17).
set -euo pipefail
cd "$(dirname "$0")/../.."

fail() { echo "ios.sh: $*" >&2; exit 1; }

version="$(node scripts/release/version.mjs)"
[[ -x .tools/odin/odin ]] || scripts/install-odin.sh

args=(--ci)
[[ -n "${BUILD_NUMBER:-}" ]] && args+=(--build-number "$BUILD_NUMBER")

build=src-tauri/gen/apple/build
out=target/release-artifacts
mkdir -p "$out"

if [[ -n "${APPLE_DEVELOPMENT_TEAM:-}" ]]; then
  pnpm tauri ios build "${args[@]}" --export-method app-store-connect "$@"
  ipa=""
  for f in "$build"/*/*.ipa "$build"/*.ipa; do
    [[ -f "$f" && ( -z "$ipa" || "$f" -nt "$ipa" ) ]] && ipa="$f"
  done
  [[ -n "$ipa" ]] || fail "no IPA under $build"
  cp "$ipa" "$out/peoples-chat-$version-ios.ipa"
  echo
  echo "Signed iOS release $version:"
  ls -l "$out/peoples-chat-$version-ios.ipa"
else
  echo "ios.sh: APPLE_DEVELOPMENT_TEAM is not set: building unsigned, up to the archive."
  pnpm tauri ios build "${args[@]}" --no-sign --archive-only "$@"
  archive="$(find "$build" -maxdepth 2 -name '*.xcarchive' -type d | head -1)"
  [[ -n "$archive" ]] || fail "no .xcarchive under $build"
  zip="$out/peoples-chat-$version-ios-unsigned.xcarchive.zip"
  rm -f "$zip"
  ditto -c -k --keepParent "$archive" "$zip"
  echo
  echo "Unsigned iOS release $version (an archive; an IPA needs a signature):"
  ls -l "$zip"
fi
