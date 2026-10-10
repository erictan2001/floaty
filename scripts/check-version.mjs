#!/usr/bin/env node
/**
 * Keeps the app version in step across its three manifests.
 *
 * package.json, src-tauri/tauri.conf.json and src-tauri/Cargo.toml each carry
 * the version, and a release that bumps only one of them ships a build whose
 * installer, updater and crate disagree. This turns that into a build error.
 *
 * Run on its own with `node scripts/check-version.mjs`.
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function read(path) {
  return readFileSync(join(root, path), "utf8");
}

const versions = {
  "package.json": JSON.parse(read("package.json")).version,
  "src-tauri/tauri.conf.json": JSON.parse(read(join("src-tauri", "tauri.conf.json"))).version,
  "src-tauri/Cargo.toml": read(join("src-tauri", "Cargo.toml")).match(
    /^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m,
  )?.[1],
};

const distinct = new Set(Object.values(versions));
if (distinct.size !== 1 || distinct.has(undefined)) {
  console.error("versions disagree:");
  for (const [file, version] of Object.entries(versions)) {
    console.error(`  ${file}: ${version ?? "(not found)"}`);
  }
  process.exit(1);
}

console.log(`versions agree: ${[...distinct][0]}`);
