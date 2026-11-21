#!/usr/bin/env bash
# Installs the Odin compiler that MDK's marmot-markdown needs at build time.
#
# The version is the one the locked MDK revision pins: this runs MDK's own installer, which
# verifies the release checksum, from the MDK checkout Cargo uses. After an MDK upgrade, run it
# again.
#
# Usage: scripts/install-odin.sh [install directory]
# The default, .tools/odin in this repository, is where .cargo/config.toml points ODIN.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
destination="${1:-$repo/.tools/odin}"
case "$destination" in
  /*) ;;
  *) destination="$PWD/$destination" ;;
esac

# The MDK checkout that holds marmot-markdown for the locked revision.
manifest="$(cd "$repo" && cargo metadata --format-version 1 --locked | python3 -c '
import json, sys
packages = json.load(sys.stdin)["packages"]
print(next(p["manifest_path"] for p in packages if p["name"] == "marmot-markdown"))
')"
mdk="$(cd "$(dirname "$manifest")/../.." && pwd)"

rm -rf "$destination"
bash "$mdk/scripts/install-odin.sh" "$destination"
