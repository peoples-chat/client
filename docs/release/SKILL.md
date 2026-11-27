---
name: release
description: Release a new version of peoples.chat for Android and iOS - bump the version, write the changelog and release notes, and push a vMAJOR.MINOR.PATCH tag that starts the release workflow
---

# Release

Release a new version: pick the version, write the changelog, commit, tag and push. The tag starts the release
workflow (`.github/workflows/release.yml`), which builds Android and iOS artefacts and drafts a GitHub release.
How versions and signing work is in `docs/release/README.md`; read it first.

Ask the person before each step that can't be undone: the version, the changelog text, and the push. Never guess either
of the first two, and never push without a yes.

## 1. Check the starting point

- Be on `main`, up to date with `origin/main` (`git fetch origin && git status`). If not, say so and stop.
- If the working tree is dirty, show `git status --short` and ask whether to go on.
- `node scripts/release/version.mjs` prints the current version and fails if the version files disagree. If it fails,
  show its output and stop: that needs a fix on its own branch first.
- `pnpm verify` must be green on this commit. If it isn't, stop.

## 2. Choose the version

- The last release is the highest tag matching `v[0-9]*.[0-9]*.[0-9]*` (`git tag --list 'v*' --sort=-v:refname`). If
  there is none, the last release is "none" and the commits to describe are all of `main`.
- Offer three choices, from the version in the files (not the tag, which may be older):
  - **Patch** (`0.1.0` → `0.1.1`): fixes only.
  - **Minor** (`0.1.0` → `0.2.0`): new features. The usual choice while we are below 1.0.
  - **Major** (`0.1.0` → `1.0.0`): only when the person says so.
  - or a version they type.
- The version must be `MAJOR.MINOR.PATCH`, minor and patch below 1000 (Android's versionCode), greater than both the
  current version and the last tag, and not an existing tag. If the current version has never been tagged
  (the first release, `0.1.0`), it may be released as it is.

## 3. Write the changelog entry

- List the commits since the last tag: `git log <last-tag>..HEAD --first-parent --format='%s'` (all of `main` for the
  first release). Our merge commits name the pull request; read the PR titles with `gh pr view <n>` where the
  subject alone is unclear.
- Write for people who use the app, not for developers. Leave out changes nobody using the app can notice
  (tests, CI, docs, refactoring) unless the person asks for them.
- Group them under `### Added`, `### Changed` and `### Fixed`; drop empty groups. One line each, plain English, with the
  issue number at the end: `- Contacts can be added from an invite link (#72)`.
- The entry:

  ```markdown
  ## [0.2.0] - 2026-12-01

  ### Added
  - …
  ```

  with today's date, and the version without `v`.
- Show it and ask the person to accept it or give the text they want instead.

## 4. Write the files

- `CHANGELOG.md`: if it doesn't exist, create it with

  ```markdown
  # Changelog

  What changed in each release of peoples.chat. Versions follow [semantic versioning](https://semver.org).
  ```

  Insert the new entry after that header, above the previous entry, with a blank line between them.
- `RELEASE_NOTES.md`: the entry's groups without its `## [x.y.z]` heading. The workflow uses it as the text of the
  GitHub release; overwrite the previous one.
- `node scripts/release/version.mjs set <version>` (skip if the version is unchanged). It updates `package.json`,
  `src-tauri/tauri.conf.json`, `Cargo.toml`, `Cargo.lock` and the iOS project's version keys, and prints the version
  back.
- Show `git diff --stat` and the changelog diff.

## 5. Commit, tag and push

Ask for a yes, then:

```sh
git add CHANGELOG.md RELEASE_NOTES.md package.json src-tauri/tauri.conf.json Cargo.toml Cargo.lock \
  src-tauri/gen/apple/project.yml src-tauri/gen/apple/peoples-chat_iOS/Info.plist
git commit -m "Release <version>"
git tag -a v<version> -m "peoples.chat <version>"
git push origin main
git push origin v<version>
```

If `main` is protected and the push is refused, push a branch `release/<version>` instead, open a pull request, and tell
the person to tag the merge commit (`git tag -a v<version> <merge-sha>` and push the tag) once it is merged. Do not tag
a commit that isn't on `main`.

## 6. Tell the person what happens next

- The release workflow is running: `gh run list --workflow release.yml --limit 1` gives its link.
- It builds the Android APK and AAB and the iOS IPA, signs them if the signing secrets are set, uploads the IPA to
  TestFlight if the App Store Connect key is set, and makes a **draft** GitHub release. Without secrets the artefacts
  are unsigned; the README lists the secrets.
- Someone has to publish the draft release, and submit the builds in Google Play Console and App Store Connect. The
  skill does neither.

## When something fails

- A git or gh command fails: show the error and stop. Don't retry with `--force`.
- The tag already exists on the remote: never move it. Ask whether to release the next patch version instead.
- The workflow fails after the tag is pushed: the tag stays. Fix forward with a new patch release.
