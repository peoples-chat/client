#!/usr/bin/env bash
# Builds the Bluetooth bridge (#135) to the path given. install.sh uses it, and so do the tests,
# which run what it builds against a fake radio only. Building asks macOS for nothing.
set -euo pipefail
[[ $# -eq 1 ]] || { echo "usage: build.sh <output file>" >&2; exit 2; }
out="$1"
here="$(cd "$(dirname "$0")" && pwd)"

# The Info.plist goes inside the program: macOS reads the bundle identifier and the reason for
# using Bluetooth from there, and ends a program that touches Bluetooth without them.
xcrun swiftc -o "$out" \
  "$here/main.swift" "$here/Bridge.swift" "$here/FakeRadioLink.swift" \
  "$here/BluetoothLink.swift" "$here/LabFile.swift" \
  -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$here/Info.plist"
# An ad-hoc signature (no certificate, no identity) that carries the same identifier.
codesign --force --sign - --identifier chat.peoples.ble-bridge "$out"
