#!/usr/bin/env node
/**
 * Keeps the examples honest about the one thing only JavaScript can answer.
 *
 * `examples/plugins/*` is what a plugin author copies from. The manifest rules an example
 * has to satisfy are `read_plugin`'s (`src-tauri/src/plugins.rs`), and
 * `plugins::tests::the_examples_validate_like_any_other_plugin` points that validator at
 * this folder in CI — so those rules live in one place and cannot drift between the two
 * languages, which is exactly what happened while this script re-implemented them:
 * tightening a rule in Rust changed nothing here.
 *
 * What is left is what no Rust test can see: the module itself. Imported the way floaty
 * imports it (an ES module, no bundler), it has to have a default export with a `mount` —
 * which also proves it does not touch the DOM at the top level, the thing that would break
 * every example at once.
 *
 * Run by `npm run build`, or on its own with `npm run check:examples`.
 */

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const EXAMPLES = join(root, "examples", "plugins");

const problems = [];

const folders = readdirSync(EXAMPLES, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .sort();

if (folders.length === 0) {
  problems.push("  examples/plugins holds no example at all");
}

for (const folder of folders) {
  const dir = join(EXAMPLES, folder);
  // The manifest is read for one thing: which file to import. Whether it is a *valid*
  // manifest is the validator's question, answered by that Rust test.
  let manifest = {};
  try {
    manifest = JSON.parse(readFileSync(join(dir, "plugin.json"), "utf8"));
  } catch (err) {
    problems.push(`  ${folder}: plugin.json is not readable JSON (${err.message})`);
    continue;
  }
  const entry =
    typeof manifest.entry === "string" && manifest.entry !== "" ? manifest.entry : "index.js";

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
    // not wrong — only a module that names a *different* one is. The folder name is
    // the id (`the_examples_validate_like_any_other_plugin` pins the two together).
    if (plugin?.kind !== undefined && plugin.kind !== folder) {
      problems.push(
        `  ${folder}: the module calls itself "${plugin.kind}", the folder says "${folder}"`,
      );
    }
  }
}

if (problems.length > 0) {
  console.error("example plugin problems:");
  for (const problem of problems) console.error(problem);
  process.exit(1);
}

console.log(
  `examples: ${folders.length} import cleanly (${folders.join(", ")}) — ` +
    "their manifests are the validator's: cargo test --lib",
);
