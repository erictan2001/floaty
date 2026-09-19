#!/usr/bin/env node
/**
 * Keeps the examples honest.
 *
 * `examples/plugins/*` is what a plugin author copies from, and nothing was checking
 * it: the examples are not built-ins, so `check-plugins.mjs` ignores them, and a
 * manifest that would be refused at install time or a module that does not import
 * would only be found by somebody following the README and hitting a wall.
 *
 * Checked here, per example: the manifest has the fields the Rust validator requires
 * (`id`, `name`, `apiVersion` inside the range this build speaks, `size`, `entry`), the
 * id is a legal plugin id, and the module imports in Node with a default export that
 * has a `mount` — which also proves it does not touch the DOM at the top level, the
 * thing that would break every example at once.
 *
 * Run by `npm run build`, or on its own with `npm run check:examples`.
 */

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const EXAMPLES = join(root, "examples", "plugins");

/** The version range `src-tauri/src/plugins.rs` accepts, read from the source. */
function apiVersions() {
  const rust = readFileSync(join(root, "src-tauri", "src", "plugins.rs"), "utf8");
  const version = rust.match(/pub const PLUGIN_API_VERSION: u32 = (\d+);/);
  const oldest = rust.match(/pub const PLUGIN_API_MIN: u32 = (\d+);/);
  if (!version || !oldest) {
    throw new Error("src-tauri/src/plugins.rs: no PLUGIN_API_VERSION / PLUGIN_API_MIN");
  }
  return { min: Number(oldest[1]), max: Number(version[1]) };
}

const problems = [];
const { min, max } = apiVersions();

const folders = readdirSync(EXAMPLES, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .sort();

if (folders.length === 0) {
  problems.push("  examples/plugins holds no example at all");
}

for (const folder of folders) {
  const dir = join(EXAMPLES, folder);
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(join(dir, "plugin.json"), "utf8"));
  } catch (err) {
    problems.push(`  ${folder}: plugin.json is not readable JSON (${err.message})`);
    continue;
  }

  const at = `examples/plugins/${folder}`;
  if (manifest.id !== folder) {
    problems.push(`  ${folder}: the folder name and the manifest id differ ("${manifest.id}")`);
  }
  if (!/^[a-z0-9_-]{2,32}$/.test(String(manifest.id ?? ""))) {
    problems.push(`  ${folder}: id must be 2-32 chars of a-z, 0-9, - and _`);
  }
  if (typeof manifest.name !== "string" || manifest.name.trim() === "") {
    problems.push(`  ${folder}: plugin.json has no name`);
  }
  const api = Number(manifest.apiVersion);
  if (!Number.isInteger(api) || api < min || api > max) {
    problems.push(`  ${folder}: apiVersion ${manifest.apiVersion} is outside ${min}..${max}`);
  }
  const size = manifest.size;
  if (!size || typeof size.w !== "number" || typeof size.h !== "number") {
    problems.push(`  ${folder}: plugin.json has no size { w, h }`);
  }
  const entry = String(manifest.entry ?? "index.js");
  if (entry.includes("/") || entry.includes("\\") || entry.includes("..")) {
    problems.push(`  ${folder}: entry must be a file name inside the folder ("${entry}")`);
  }

  // The module itself, imported the way floaty imports it (an ES module, no bundler).
  const module = await import(pathToFileURL(join(dir, entry)).href).catch((err) => {
    problems.push(`  ${folder}: ${entry} did not import (${err.message})`);
    return null;
  });
  if (module) {
    const plugin = module.default;
    if (!plugin || typeof plugin.mount !== "function") {
      problems.push(`  ${folder}: ${entry} has no default export with a mount() function`);
    }
    // The loader takes the kind from the manifest, so a module that names its own is
    // not wrong — only a module that names a *different* one is.
    if (plugin?.kind !== undefined && plugin.kind !== manifest.id) {
      problems.push(`  ${folder}: the module calls itself "${plugin.kind}", the manifest says "${manifest.id}"`);
    }
  }
  void at;
}

if (problems.length > 0) {
  console.error("example plugin problems:");
  for (const problem of problems) console.error(problem);
  process.exit(1);
}

console.log(`examples: ${folders.length} import cleanly and match the manifest rules (${folders.join(", ")})`);
