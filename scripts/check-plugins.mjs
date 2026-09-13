#!/usr/bin/env node
/**
 * Keeps the two halves of the plugin system in step.
 *
 * The backend manifest (src-tauri/src/plugins.rs) and the frontend registry
 * (src/widgets/plugin.ts) each list the widget kinds — they have to, they are
 * different languages — and a kind that exists in only one of them fails in a
 * way that is easy to miss: a widget that mounts nothing, or a plugin that never
 * appears in settings. This turns that into a build error.
 *
 * Run by `npm run build`, or on its own with `npm run check:plugins`.
 */

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE = join(root, "src", "widgets");

function read(path) {
  return readFileSync(path, "utf8");
}

/** Kinds in the order the Rust manifest declares them. */
function rustKinds() {
  const rust = read(join(root, "src-tauri", "src", "plugins.rs"));
  return [...rust.matchAll(/^\s{8}id: "([a-z0-9_-]+)",$/gm)].map((m) => m[1]);
}

/** Kinds in the order `BUILTINS` lists them, resolved through their modules. */
function typescriptKinds() {
  const registry = read(join(SOURCE, "plugin.ts"));
  const block = registry.match(/const BUILTINS: FloatyPlugin\[\] = \[([\s\S]*?)\];/);
  if (!block) {
    throw new Error("src/widgets/plugin.ts: no BUILTINS list found");
  }
  const identifiers = [...block[1].matchAll(/^\s*([A-Za-z_$][\w$]*),$/gm)].map((m) => m[1]);

  const modules = new Map();
  for (const file of readdirSync(SOURCE)) {
    if (!file.endsWith(".ts") || file === "plugin.ts") continue;
    const source = read(join(SOURCE, file));
    for (const m of source.matchAll(/export const (\w+): FloatyPlugin = \{\s*\n\s*kind: "([a-z0-9_-]+)"/g)) {
      modules.set(m[1], { kind: m[2], file });
    }
  }

  return identifiers.map((identifier) => {
    const found = modules.get(identifier);
    if (!found) {
      throw new Error(`src/widgets/plugin.ts: BUILTINS lists "${identifier}" but no module exports it as a FloatyPlugin with a kind`);
    }
    return { ...found, identifier };
  });
}

const rust = rustKinds();
const ts = typescriptKinds();
const tsKinds = ts.map((t) => t.kind);
const problems = [];

// The manifest crosses the language boundary as JSON, so a renamed field is a
// silent fallback rather than a compile error. Pin the ones the frontend reads
// (the Rust side asserts the same list in `the_payload_carries_the_keys_...`).
const WIRE_FIELDS = [
  "default_size",
  "desktop_item",
  "path_key",
  "noun",
  "group",
  "add_label",
  "layout_priority",
  "source",
  "entry",
];
const manifestSource = read(join(SOURCE, "pluginManifest.ts"));
const forgotten = WIRE_FIELDS.filter((field) => !manifestSource.includes(field));
if (forgotten.length > 0) {
  problems.push(
    `  src/widgets/pluginManifest.ts no longer mentions [${forgotten.join(", ")}] — the backend still sends them`,
  );
}

for (const kind of rust) {
  if (!tsKinds.includes(kind)) {
    problems.push(`  ${kind}: in src-tauri/src/plugins.rs but no frontend module mounts it`);
  }
}
for (const { kind, file } of ts) {
  if (!rust.includes(kind)) {
    problems.push(`  ${kind}: in src/widgets/${file} but missing from src-tauri/src/plugins.rs`);
  }
}
if (rust.join(",") !== tsKinds.join(",") && problems.length === 0) {
  problems.push(`  order differs: rust [${rust.join(", ")}] vs BUILTINS [${tsKinds.join(", ")}]`);
}

if (problems.length > 0) {
  console.error(`plugin manifest mismatch (${rust.length} backend, ${ts.length} frontend):`);
  for (const problem of problems) console.error(problem);
  process.exit(1);
}

console.log(`plugins: ${rust.length} kinds agree (${rust.join(", ")})`);
