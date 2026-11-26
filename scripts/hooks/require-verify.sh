#!/usr/bin/env bash
# Stop hook: "done" means verified. `pnpm verify` writes HEAD to .factory/verified on success.
input=$(cat)
# Second stop in a row: let it end, so the hook can never loop forever.
if [[ "$(jq -r '.stop_hook_active // false' <<<"$input")" == "true" ]]; then exit 0; fi
# Read-only sessions (the reviewer runs in plan mode) change nothing, so there is nothing to verify.
if [[ "$(jq -r '.permission_mode // ""' <<<"$input")" == "plan" ]]; then exit 0; fi
root=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
head=$(git -C "$root" rev-parse HEAD)
verified=$(cat "$root/.factory/verified" 2>/dev/null || true)
if [[ "$verified" != "$head" ]] || [[ -n "$(git -C "$root" status --porcelain --untracked-files=no)" ]]; then
  echo "Run pnpm verify and commit before finishing (verified: ${verified:-none}, HEAD: $head)" >&2
  exit 2
fi
exit 0
