#!/usr/bin/env bash
# The single definition of "works": agents, the Stop hook and CI run this.
# scripts/verify.sh          every layer, then writes HEAD to .factory/verified
# scripts/verify.sh --quick  everything except desktop e2e (agents loop on this)
set -euo pipefail
cd "$(dirname "$0")/.."
quick=0; [[ "${1:-}" == "--quick" ]] && quick=1
step() { echo; echo "==> $*"; }

[[ -x .tools/odin/odin ]] || { step "Installing Odin"; scripts/install-odin.sh; }

step "Rust format";        cargo fmt --all --check
step "Rust clippy (core)"; cargo clippy -p peoples-chat-core --all-targets -- -D warnings
step "Rust clippy (shell)";cargo clippy -p peoples-chat --all-targets -- -D warnings
step "Rust tests";         cargo test -p peoples-chat-core
step "Type check";         pnpm check
step "Front-end tests";    pnpm test

if (( quick == 0 )); then
  if [[ -f e2e/wdio.conf.ts ]]; then
    step "Desktop e2e"; pnpm exec wdio run e2e/wdio.conf.ts
  else
    echo "!! Desktop e2e harness not set up yet (e2e/wdio.conf.ts missing): see the factory bootstrap issue." >&2
  fi
fi

mkdir -p .factory
if (( quick == 0 )); then git rev-parse HEAD > .factory/verified; fi
echo; echo "verify: all steps passed$( ((quick)) && echo ' (quick)')"
