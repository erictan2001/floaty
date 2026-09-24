#!/usr/bin/env node
/**
 * Bring main's version up to the tag that was just released.
 *
 * ADR 0003 (docs/adr/0003-two-version-numbers.md) says main always states the *last
 * released* version, not the one being worked on. `scripts/set-version.mjs` is how a
 * version gets written into the three files; this is what decides that it should be
 * written, and it is called from the release workflow once the release is published, so
 * the policy holds without anyone remembering to do it.
 *
 *   node scripts/sync-main-version.mjs v0.2.0
 *   node scripts/sync-main-version.mjs --force v0.1.0    (withdrawal repair only)
 *
 * Idempotent and forward-only. A tag main already states is a no-op, and a tag *older*
 * than what main states is refused rather than written — a `workflow_dispatch` rebuild of
 * v0.1.1 must not drag main back to 0.1.1. `--force` is for the one case that has to move
 * main backwards on purpose: a release withdrawn under ADR 0002
 * (docs/adr/0002-withdraw-bad-releases.md), where the last *live* release is an older one.
 * Nothing automatic ever passes it.
 *
 * It writes nothing itself. The version goes to set-version.mjs, which owns the three
 * files and is already idempotent per file, so a second and third file that disagree with
 * package.json are corrected on the way through.
 *
 * Exit 0 = main is (now) at the tag, or was deliberately left ahead.
 * Exit 1 = the argument is not a version, or set-version.mjs failed.
 */

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const args = process.argv.slice(2);
const force = args.includes("--force");
const tag = (args.find((argument) => !argument.startsWith("-")) ?? "").trim();
const version = tag.replace(/^v/, "");
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`sync-main-version: "${tag}" is not a v<version> tag (expected e.g. v0.2.0)`);
  process.exit(1);
}

/** The numeric triple, which is all this repo's versions are. */
function core(v) {
  return v.split(/[-+]/)[0].split(".").map(Number);
}

function compare(a, b) {
  const [x, y] = [core(a), core(b)];
  for (let i = 0; i < 3; i += 1) {
    if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
  }
  return 0;
}

// package.json is the one that is always read; the other two follow it through
// set-version.mjs.
const current = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;
const order = compare(version, current);

if (order < 0 && !force) {
  console.log(
    `sync-main-version: main states ${current}, ahead of ${version}; leaving it ` +
      "(a rebuild of an old release must not drag main back)",
  );
  process.exit(0);
}
if (order < 0) {
  console.log(`sync-main-version: --force: main states ${current}, moving it back to ${version}`);
} else if (order === 0) {
  console.log(`sync-main-version: main states ${current}; checking the other two files agree`);
} else {
  console.log(`sync-main-version: main states ${current}; writing ${version} (from ${tag})`);
}

// set-version.mjs re-serialises the two JSON files with `JSON.stringify`, which emits
// `\n`. package.json and src-tauri/tauri.conf.json are checked in with CRLF — this is a
// Windows app and there is no .gitattributes saying otherwise — so committing its output
// would rewrite every line of both files to change one. Note each file's own line endings
// now and put them back after, so the bump commit is one changed line per file.
// (Cargo.toml is edited by regex and keeps its endings; it is LF anyway.)
const JSON_FILES = ["package.json", "src-tauri/tauri.conf.json"];
const crlf = new Map(
  JSON_FILES.map((file) => [file, readFileSync(join(root, file), "utf8").includes("\r\n")]),
);

execFileSync(process.execPath, [join(root, "scripts", "set-version.mjs"), version], {
  cwd: root,
  stdio: "inherit",
});

for (const file of JSON_FILES) {
  const path = join(root, file);
  const text = readFileSync(path, "utf8");
  if (crlf.get(file) && !text.includes("\r\n")) {
    writeFileSync(path, text.replace(/\n/g, "\r\n"));
  }
}
