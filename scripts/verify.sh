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
step "Rust clippy (shell, e2e build)"; cargo clippy -p peoples-chat --all-targets --features e2e -- -D warnings
step "Rust clippy (fake radio)"; cargo clippy -p peoples-chat-fake-radio --all-targets -- -D warnings
step "Rust tests";         cargo test -p peoples-chat-core
step "Rust tests (fake radio)";  cargo test -p peoples-chat-fake-radio
step "Type check";         pnpm check && pnpm e2e:check
step "Front-end tests";    pnpm test

if (( quick == 0 )); then
  step "Desktop e2e (builds the app with --features e2e)"; pnpm e2e
fi

mkdir -p .factory
if (( quick == 0 )); then git rev-parse HEAD > .factory/verified; fi
echo; echo "verify: all steps passed$( ((quick)) && echo ' (quick)')"
