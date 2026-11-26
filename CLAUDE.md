# peoples.chat: notes for agents

An everyday encrypted messenger that keeps working over LoRa radio when the internet is gone.
It is shown in a showcase on macOS desktop and iOS. Read [README.md](README.md) for the layout.

## Architecture
- `core/` (`peoples-chat-core`): all protocol logic, built on MDK ([docs/mdk.md](docs/mdk.md)). No Tauri or UI code.
- `src-tauri/`: Tauri 2 shell; forwards commands to the core.
- `src/`: React + TypeScript + Vite. Shows state and sends commands, nothing more.
  `src/core/` is the boundary: commands, events, the real bridge and a mock core.
- The boundary between core and web view is specified in [docs/core-interface.md](docs/core-interface.md).
- Radio: [docs/radio.md](docs/radio.md), [docs/radio-format-v0.md](docs/radio-format-v0.md).

## Conventions
- pnpm, never npm. Rust toolchain pinned in `rust-toolchain.toml`.
- `cargo fmt`, clippy with `-D warnings`, `tsc` must be clean.
- Select UI elements in tests by `data-testid`, never by CSS structure or text. Add the testid when you add the test.
- Desktop e2e: WebdriverIO specs in `e2e/` (`pnpm e2e` builds the app with `--features e2e` and runs them; each run
  gets an empty data folder). Screenshots: `browser.saveScreenshot("evidence/<issue>/<step>.png")`.
- No new dependencies without a reason in the PR body.

## Definition of done
1. `pnpm verify` is green (it stamps `.factory/verified`; the Stop hook checks it). Loop on `pnpm verify:quick`.
2. Every acceptance criterion has a test, named after the issue (`e2e/issue-42-onboarding.spec.ts`, or a Rust/Vitest test).
3. Screenshots the issue asks for are on the `evidence` branch under `evidence/<issue>/` and linked in the PR.
4. The PR body maps each criterion to its test and screenshot, and starts with `Closes #<issue>`.

## Never touch
Signing config, `.github/`, `CLAUDE.md`, `.claude/`, `scripts/hooks/`, other issues' branches,
radio firmware, frequency or preset.
