#!/usr/bin/env bash
# Writes the launchd job of the Bluetooth bridge (#135) to a file. It only writes the file;
# install.sh loads it.
#
#   job.sh <plist file> <label> <output log> <keep alive after a crash: true|false> <program> [argument]...
set -euo pipefail
[[ $# -ge 5 ]] || { sed -n '2,5p' "$0" >&2; exit 2; }
file="$1" label="$2" output="$3" keep="$4"
shift 4

string() {
  printf '<string>%s</string>\n' \
    "$(printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')"
}

{
  echo '<?xml version="1.0" encoding="UTF-8"?>'
  echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
  echo '<plist version="1.0"><dict>'
  echo '<key>Label</key>'
  string "$label"
  echo '<key>ProgramArguments</key><array>'
  for argument in "$@"; do string "$argument"; done
  echo '</array>'
  echo '<key>RunAtLoad</key><true/>'
  # Started again after a crash only: not after it stopped because Bluetooth isn't allowed.
  # A job that runs once has no KeepAlive at all, because launchd reads `Crashed` false as
  # "start it again after every other exit".
  if [[ "$keep" == true ]]; then
    echo '<key>KeepAlive</key><dict><key>Crashed</key><true/></dict>'
  fi
  echo '<key>StandardOutPath</key>'
  string "$output"
  echo '<key>StandardErrorPath</key>'
  string "$output"
  echo '</dict></plist>'
} >"$file"
plutil -lint -s "$file"
