// #144: Android and iOS release builds. Checks the version numbering, the proposed release
// workflow, the release skill and the local build scripts.
import { accessSync, constants, copyFileSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  VERSION_FILES,
  androidVersionCode,
  currentVersion,
  parseVersion,
  readVersions,
  setVersion,
  versionFromTag,
} from "./version.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const read = (file: string) => readFileSync(join(root, file), "utf8");

/** A copy of just the version files, to change without touching the checkout. */
function copyOfVersionFiles() {
  const dir = mkdtempSync(join(tmpdir(), "issue-144-"));
  for (const file of new Set(VERSION_FILES.map((f) => f.file))) {
    mkdirSync(dirname(join(dir, file)), { recursive: true });
    copyFileSync(join(root, file), join(dir, file));
  }
  return dir;
}

describe("issue 144: version numbering", () => {
  it("finds one version in every file that carries it, and they agree", () => {
    const found = readVersions(root);
    const files = new Set(found.map((f) => f.file));
    expect(files).toEqual(
      new Set([
        "package.json",
        "src-tauri/tauri.conf.json",
        "Cargo.toml",
        "Cargo.lock",
        "src-tauri/gen/apple/project.yml",
        "src-tauri/gen/apple/peoples-chat_iOS/Info.plist",
      ]),
    );
    // Two keys each in project.yml and Info.plist, four crates in Cargo.lock.
    expect(found).toHaveLength(3 + 4 + 2 + 2);
    const version = currentVersion(root);
    expect(JSON.parse(read("package.json")).version).toBe(version);
    expect(JSON.parse(read("src-tauri/tauri.conf.json")).version).toBe(version);
  });

  it("sets a new version everywhere and nothing else", () => {
    const dir = copyOfVersionFiles();
    const before = new Map([...new Set(VERSION_FILES.map((f) => f.file))].map((f) => [f, read(f)]));
    const old = currentVersion(root);

    expect(setVersion(dir, "2.13.7")).toBe("2.13.7");
    expect(new Set(readVersions(dir).map((f) => f.version))).toEqual(new Set(["2.13.7"]));
    expect(JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).version).toBe("2.13.7");

    // Setting the old version back restores every file byte for byte.
    setVersion(dir, old);
    for (const [file, text] of before) expect(readFileSync(join(dir, file), "utf8"), file).toBe(text);
  });

  it("leaves other crates' versions in Cargo.lock alone", () => {
    const dir = copyOfVersionFiles();
    const otherVersions = (text: string) =>
      text.split("\n[[package]]\n").filter((p) => !/^name = "(peoples-chat[\w-]*|tauri-plugin-share-sheet)"$/m.test(p));
    setVersion(dir, "9.9.9");
    expect(otherVersions(readFileSync(join(dir, "Cargo.lock"), "utf8"))).toEqual(otherVersions(read("Cargo.lock")));
  });

  it("reports files that disagree", () => {
    const dir = copyOfVersionFiles();
    const file = join(dir, "src-tauri/tauri.conf.json");
    writeFileSync(file, readFileSync(file, "utf8").replace(/"version": "[^"]*"/, '"version": "0.0.9"'));
    expect(() => currentVersion(dir)).toThrow(/src-tauri\/tauri\.conf\.json: 0\.0\.9/);
  });

  it("accepts only MAJOR.MINOR.PATCH, and tags of the form vMAJOR.MINOR.PATCH", () => {
    expect(parseVersion("1.2.3")).toEqual({ major: 1, minor: 2, patch: 3 });
    for (const bad of ["1.2", "1.2.3-beta", "v1.2.3", "01.2.3", "1.1000.0", "1.0.1000", ""]) {
      expect(() => parseVersion(bad), bad).toThrow();
    }
    expect(versionFromTag("v0.2.0")).toBe("0.2.0");
    expect(versionFromTag("refs/tags/v1.0.4")).toBe("1.0.4");
    expect(() => versionFromTag("dev-abc123")).toThrow();
    expect(() => setVersion(copyOfVersionFiles(), "1.2")).toThrow();
  });

  it("gives Android a versionCode that grows with the version, as Tauri derives it", () => {
    expect(androidVersionCode("0.1.0")).toBe(1000);
    expect(androidVersionCode("1.2.3")).toBe(1_002_003);
    expect(androidVersionCode("0.1.1")).toBeGreaterThan(androidVersionCode("0.1.0"));
    expect(androidVersionCode("0.2.0")).toBeGreaterThan(androidVersionCode("0.1.999"));
    expect(androidVersionCode("1.0.0")).toBeGreaterThan(androidVersionCode("0.999.999"));
    // tauri.conf.json must not pin versionCode, or it would stop following the version.
    expect(JSON.parse(read("src-tauri/tauri.conf.json")).bundle?.android?.versionCode).toBeUndefined();
  });
});

describe("issue 144: proposed release workflow", () => {
  const workflow = read("docs/release/build.yml");
  const readme = read("docs/release/README.md");

  it("runs on version tags and by hand", () => {
    expect(workflow).toMatch(/^on:\n {2}push:\n {4}tags: \["v\*"\]\n {2}workflow_dispatch:$/m);
  });

  it("builds Android and iOS with the repository's own scripts, after setting the version from the tag", () => {
    for (const job of ["android", "ios"]) {
      const body = workflow.split(new RegExp(`^ {2}${job}:$`, "m"))[1]?.split(/^ {2}\w+:$/m)[0] ?? "";
      expect(body, job).toContain('node scripts/release/version.mjs from-tag "$GITHUB_REF_NAME"');
      expect(body, job).toContain(`scripts/release/${job}.sh`);
    }
  });

  it("documents every secret it reads, and reads no key from the repository", () => {
    const used = new Set([...workflow.matchAll(/secrets\.(\w+)/g)].map((m) => m[1]));
    expect(used.size).toBeGreaterThan(0);
    const documented = new Set([...readme.matchAll(/^\| `(\w+)` \|/gm)].map((m) => m[1]));
    expect(documented).toEqual(used);
    // Keys from secrets are written only outside the checkout, and deleted after use.
    const writes = [...workflow.matchAll(/echo -n "\$(\w+)".*> "?([^"\s]+)"?$/gm)];
    expect(writes.map((m) => m[1]).sort()).toEqual(["ANDROID_KEYSTORE_BASE64", "APPLE_API_KEY"]);
    for (const [, , target] of writes) {
      expect(target).toMatch(/^(\$keystore|~\/private_keys\/.*)$/);
      expect(workflow).toContain(`rm ${target.startsWith("$") ? `"${target}"` : target}`);
    }
    expect(workflow).toContain('keystore="$RUNNER_TEMP/upload.jks"');
  });

  it("only drafts a GitHub release, from tags", () => {
    const release = workflow.split(/^ {2}release:$/m)[1] ?? "";
    expect(release).toContain("if: startsWith(github.ref, 'refs/tags/v')");
    expect(release).toContain("draft: true");
  });
});

describe("issue 144: release skill and local scripts", () => {
  it("has a release skill that uses the version script and asks before pushing", () => {
    const skill = read("docs/release/SKILL.md");
    expect(skill).toMatch(/^---\nname: release\ndescription: .+\n---\n/);
    expect(skill).toContain("node scripts/release/version.mjs set <version>");
    expect(skill).toContain("git push origin v<version>");
    expect(skill).toMatch(/Ask for a yes, then:/);
  });

  it("offers pnpm release:android and release:ios, backed by executable scripts", () => {
    const scripts = JSON.parse(read("package.json")).scripts;
    expect(scripts["release:android"]).toBe("scripts/release/android.sh");
    expect(scripts["release:ios"]).toBe("scripts/release/ios.sh");
    for (const file of ["android.sh", "ios.sh", "version.mjs"]) {
      accessSync(join(root, "scripts/release", file), constants.X_OK);
    }
  });

  it("builds unsigned by default: no signing in Gradle, --no-sign for iOS without a team", () => {
    expect(read("src-tauri/gen/android/app/build.gradle.kts")).not.toMatch(/signingConfig/);
    const ios = read("scripts/release/ios.sh");
    expect(ios).toContain("--no-sign");
    expect(read("scripts/release/android.sh")).toContain("pnpm tauri android build --ci --apk --aab");
  });
});
