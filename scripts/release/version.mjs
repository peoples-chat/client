#!/usr/bin/env node
// @ts-check
// The app's version lives in several files. This script reads them, checks that they agree and
// sets them all at once (#144). docs/release/README.md explains the numbering.
//
//   node scripts/release/version.mjs              print the version; fail if the files disagree
//   node scripts/release/version.mjs set 0.2.0    write 0.2.0 into every file
//   node scripts/release/version.mjs from-tag v0.2.0
//                                                 the same, from a release tag (CI uses this)
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Our own crates in Cargo.lock. They all take the workspace version. */
const WORKSPACE_CRATES = [
  "peoples-chat",
  "peoples-chat-core",
  "peoples-chat-fake-radio",
  "tauri-plugin-share-sheet",
];

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

/**
 * Each place the version is written, as a pattern whose first group is the version.
 * `all` means every match must carry it (Info.plist has two keys).
 * @type {{ file: string, pattern: RegExp, all?: boolean }[]}
 */
export const VERSION_FILES = [
  { file: "package.json", pattern: /^ {2}"version": "([^"]*)"/m },
  { file: "src-tauri/tauri.conf.json", pattern: /^ {2}"version": "([^"]*)"/m },
  { file: "Cargo.toml", pattern: /^\[workspace\.package\]\n(?:[^[\n].*\n)*?version = "([^"]*)"/m },
  ...WORKSPACE_CRATES.map((name) => ({
    file: "Cargo.lock",
    pattern: new RegExp(`^name = "${name}"\\nversion = "([^"]*)"`, "m"),
  })),
  {
    file: "src-tauri/gen/apple/project.yml",
    pattern: /^ +CFBundle(?:ShortVersionString|Version): "?([^"\n]*)"?$/gm,
    all: true,
  },
  {
    file: "src-tauri/gen/apple/peoples-chat_iOS/Info.plist",
    pattern: /<key>CFBundle(?:ShortVersionString|Version)<\/key>\s*<string>([^<]*)<\/string>/g,
    all: true,
  },
];

/** @param {string} version */
export function parseVersion(version) {
  const m = SEMVER.exec(version);
  if (!m) throw new Error(`"${version}" is not a version of the form MAJOR.MINOR.PATCH`);
  const [major, minor, patch] = m.slice(1).map(Number);
  if (minor > 999 || patch > 999) throw new Error(`"${version}": minor and patch must be below 1000`);
  return { major, minor, patch };
}

/**
 * Android's versionCode, as Tauri derives it when `bundle.android.versionCode` is unset.
 * @param {string} version
 */
export function androidVersionCode(version) {
  const { major, minor, patch } = parseVersion(version);
  return major * 1_000_000 + minor * 1000 + patch;
}

/** @param {string} tag */
export function versionFromTag(tag) {
  const version = tag.replace(/^refs\/tags\//, "").replace(/^v/, "");
  parseVersion(version);
  return version;
}

/**
 * Every version found, by file.
 * @param {string} root
 * @returns {{ file: string, version: string }[]}
 */
export function readVersions(root) {
  return VERSION_FILES.flatMap(({ file, pattern, all }) => {
    const text = readFileSync(join(root, file), "utf8");
    const found = all ? [...text.matchAll(pattern)].map((m) => m[1]) : [pattern.exec(text)?.[1]];
    if (found.length === 0 || found.some((v) => v === undefined)) {
      throw new Error(`${file}: no version found (${pattern})`);
    }
    return found.map((version) => ({ file, version: /** @type {string} */ (version) }));
  });
}

/**
 * The version all files agree on; throws if they don't.
 * @param {string} root
 */
export function currentVersion(root) {
  const found = readVersions(root);
  const versions = new Set(found.map((f) => f.version));
  if (versions.size !== 1) {
    const list = found.map((f) => `  ${f.file}: ${f.version}`).join("\n");
    throw new Error(`the version files disagree:\n${list}`);
  }
  return found[0].version;
}

/**
 * @param {string} root
 * @param {string} version
 */
export function setVersion(root, version) {
  parseVersion(version);
  /** @type {Map<string, string>} */
  const texts = new Map();
  for (const { file, pattern } of VERSION_FILES) {
    const text = texts.get(file) ?? readFileSync(join(root, file), "utf8");
    const global = pattern.global ? pattern : new RegExp(pattern.source, pattern.flags + "g");
    // The version is the last thing each pattern captures, so its last occurrence in the match is it.
    const replaced = text.replace(global, (match, /** @type {string} */ old) => {
      const at = match.lastIndexOf(old);
      return match.slice(0, at) + version + match.slice(at + old.length);
    });
    texts.set(file, replaced);
  }
  for (const [file, text] of texts) writeFileSync(join(root, file), text);
  return currentVersion(root);
}

function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
  const [command, arg] = process.argv.slice(2);
  try {
    if (!command) {
      console.log(currentVersion(root));
    } else if (command === "set" && arg) {
      console.log(setVersion(root, arg));
    } else if (command === "from-tag" && arg) {
      console.log(setVersion(root, versionFromTag(arg)));
    } else {
      console.error("usage: version.mjs [set <X.Y.Z> | from-tag <vX.Y.Z>]");
      process.exit(2);
    }
  } catch (e) {
    console.error(`version.mjs: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
