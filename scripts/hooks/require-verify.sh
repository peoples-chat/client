#!/usr/bin/env bash
# Stop hook: "done" means verified. `pnpm verify` writes HEAD to .factory/verified on success.
input=$(cat)
if [[ "$(jq -r '.stop_hook_active // false' <<<"$input")" == "true" ]]; then exit 0; fi
root=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
head=$(git -C "$root" rev-parse HEAD)
verified=$(cat "$root/.factory/verified" 2>/dev/null || true)
if [[ "$verified" != "$head" ]] || [[ -n "$(git -C "$root" status --porcelain --untracked-files=no)" ]]; then
  echo "Run pnpm verify and commit before finishing (verified: ${verified:-none}, HEAD: $head)" >&2
  exit 2
fi
exit 0
