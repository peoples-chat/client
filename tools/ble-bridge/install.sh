#!/usr/bin/env bash
# Installs the Bluetooth bridge (#135) for the radio lab. A person runs it; `verify` never does.
#
#   tools/ble-bridge/install.sh                    builds it once and starts its launchd job
#   tools/ble-bridge/install.sh --allow-bluetooth  once, at the Mac: macOS asks, click Allow
#   tools/ble-bridge/install.sh --rebuild          builds it again (macOS then asks again)
#   tools/ble-bridge/install.sh --uninstall        stops the job and removes both
#
# Environment: BLE_BRIDGE_BIN (default ~/factory/bin/ble-bridge), RADIOS_TOML (default
# ~/factory/radios.toml), BLE_BRIDGE_LOG (default ~/factory/logs/ble-bridge.log).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
bin="${BLE_BRIDGE_BIN:-$HOME/factory/bin/ble-bridge}"
lab="${RADIOS_TOML:-$HOME/factory/radios.toml}"
log="${BLE_BRIDGE_LOG:-$HOME/factory/logs/ble-bridge.log}"
label="chat.peoples.factory.ble-bridge"
agents="$HOME/Library/LaunchAgents"
plist="$agents/$label.plist"
domain="gui/$(id -u)"

rebuild=0
allow=0
uninstall=0
for argument in "$@"; do
  case "$argument" in
    --rebuild) rebuild=1 ;;
    --allow-bluetooth) allow=1 ;;
    --uninstall) uninstall=1 ;;
    *) sed -n '2,10p' "$0" >&2; exit 2 ;;
  esac
done

stop() {
  launchctl bootout "$domain/$label" 2>/dev/null || true
  # bootout returns before the job is gone.
  for _ in $(seq 50); do
    launchctl print "$domain/$label" >/dev/null 2>&1 || return 0
    sleep 0.1
  done
}

if ((uninstall)); then
  stop
  rm -f "$plist" "$bin"
  echo "Removed $plist and $bin."
  exit 0
fi

[[ -f "$lab" ]] || { echo "No radio lab file at $lab. Set RADIOS_TOML." >&2; exit 1; }
mkdir -p "$(dirname "$bin")" "$(dirname "$log")" "$agents"
stop

# Built once: macOS ties its permission to this exact program.
if ((rebuild)) || [[ ! -x "$bin" ]]; then
  bash "$here/build.sh" "$bin"
  echo "Built $bin."
else
  echo "$bin is already built (--rebuild builds it again)."
fi

if ((allow)); then
  # Run as its own launchd job, so that macOS asks for the bridge and not for the terminal.
  temporary="$(mktemp -d)"
  bash "$here/job.sh" "$temporary/$label.plist" "$label" "$temporary/output" false \
    "$bin" --request-permission
  launchctl bootstrap "$domain" "$temporary/$label.plist"
  echo "Click Allow in the dialog on this Mac's screen (within 2 minutes)."
  for _ in $(seq 250); do
    grep -q 'Bluetooth permission:' "$temporary/output" 2>/dev/null && break
    sleep 0.5
  done
  stop
  cat "$temporary/output" 2>/dev/null || true
  if ! grep -q 'Bluetooth permission: allowed' "$temporary/output" 2>/dev/null; then
    echo "Bluetooth isn't allowed yet; the bridge wasn't started." >&2
    exit 1
  fi
fi

bash "$here/job.sh" "$plist" "$label" "$log" true "$bin" --lab "$lab"
: >"$log"
launchctl bootstrap "$domain" "$plist"
sleep 3
cat "$log"
echo "The job is $label; its output is in $log."
if grep -q "hasn't allowed" "$log"; then
  echo "Next: tools/ble-bridge/install.sh --allow-bluetooth" >&2
  exit 1
fi
