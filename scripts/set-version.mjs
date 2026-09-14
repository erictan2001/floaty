// Set the app version everywhere it is written down.
//
// A release is a tag, and the tag is the only place the version should have to be
// typed: tauri.conf.json names the installers and shows in the exe's properties,
// Cargo.toml names the binary, package.json keeps the node tooling honest. When
// those disagree with the tag, the release page says one version and the file it
// hands you says another.
//
//   node scripts/set-version.mjs 0.2.0
//
// The release workflow runs this with the version from the pushed tag.

import { readFileSync, writeFileSync } from "node:fs";

const version = (process.argv[2] ?? "").trim();
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`set-version: "${version}" is not a version (expected e.g. 0.2.0)`);
  process.exit(1);
}

const edited = [];

for (const file of ["package.json", "src-tauri/tauri.conf.json"]) {
  const json = JSON.parse(readFileSync(file, "utf8"));
  if (json.version === version) continue;
  json.version = version;
  writeFileSync(file, `${JSON.stringify(json, null, 2)}\n`);
  edited.push(file);
}

// Only the package's own version: the first one, which is above every [section]
// (a dependency's version line lives inside a section and is indented).
const cargoFile = "src-tauri/Cargo.toml";
const cargo = readFileSync(cargoFile, "utf8");
const next = cargo.replace(/^version = ".*"$/m, `version = "${version}"`);
if (next !== cargo) {
  writeFileSync(cargoFile, next);
  edited.push(cargoFile);
}

console.log(
  edited.length
    ? `set-version: ${version} written to ${edited.join(", ")}`
    : `set-version: ${version} was already set`,
);
