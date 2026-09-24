#!/usr/bin/env node
/**
 * The guards that run *before* a release is published.
 *
 * A bad release is withdrawn, not kill-switched — there is no server behind the
 * updater, so nothing can be turned off remotely (docs/adr/0002-withdraw-bad-releases.md).
 * That only works if the bad release never reaches the feed in the first place, so this
 * runs between "the bundles are built and uploaded" and "the release is published":
 *
 *   1. the artifact the manifest points at is present and not zero bytes;
 *   2. the `.sig` that belongs to it is present;
 *   3. the manifest's version equals the git tag it is being published under;
 *   4. the manifest's signature verifies, against the public key in
 *      `src-tauri/tauri.conf.json`, over the artifact's bytes;
 *   5. the address in the manifest points at *this* release and this tag — a feed
 *      entry left over from an older release is the failure this catches.
 *
 * Every check prints what it looked at and the value it saw, so a failure says which
 * value was wrong rather than only that something was.
 *
 *   node scripts/verify-updater-feed.mjs --tag v0.2.0 --repo erictan2001/floaty \
 *     --require-platforms windows-x86_64,windows-aarch64 [--publish]
 *
 * `--publish` takes the release out of draft once every check above has passed; it is
 * the same invocation that did the checking, so nothing can be published unverified.
 *
 * Options
 *   --tag <tag>                the tag being published (required; default $TAG)
 *   --repo <owner/name>        the repository (default $GITHUB_REPOSITORY)
 *   --artifacts <list|json>    built files to smoke-test locally (default $ARTIFACTS)
 *   --manifest <path>          check a `latest.json` on disk instead of the release's
 *   --release-json <path>      read the release payload from a file instead of the API
 *   --require-platforms <a,b>  manifest keys that must exist (e.g. windows-aarch64)
 *   --config <path>            tauri.conf.json (default src-tauri/tauri.conf.json)
 *   --no-download              verify against the local artifacts instead of downloading
 *   --publish                  publish the draft release once the checks pass
 *   --dry-run                  with --publish: say what would be published, do nothing
 *   --help
 *
 * Reads $GITHUB_TOKEN (or $GH_TOKEN) to see a draft release and its assets; without it
 * only published releases and public assets can be read.
 */

import { createHash, createPublicKey, verify as ed25519Verify } from "node:crypto";
import { closeSync, existsSync, openSync, readFileSync, readSync, statSync } from "node:fs";
import { basename, resolve } from "node:path";

const MINISIGN_SIG_BYTES = 74;
const MINISIGN_KEY_BYTES = 42;
const BLAKE2B_512 = 64;

// ---------------------------------------------------------------- reporting --

let checks = 0;
let failures = 0;

const section = (title) => console.log(`\n${title}`);

function ok(label, detail) {
  checks += 1;
  console.log(`  ok    ${label}${detail === undefined ? "" : ` — ${detail}`}`);
}

function bad(label, detail) {
  checks += 1;
  failures += 1;
  const line = `${label}${detail === undefined ? "" : ` — ${detail}`}`;
  console.log(`  FAIL  ${line}`);
  // an annotation, so a failed release says why on the run page as well as in the log
  console.log(`::error::verify-updater-feed: ${line}`);
}

function note(label, detail) {
  console.log(`  ..    ${label}${detail === undefined ? "" : ` — ${detail}`}`);
}

// ------------------------------------------------------------------- minisign --

/** base64, strictly — a typo in a key should not decode to something plausible. */
function fromBase64(text, what) {
  const clean = String(text ?? "").replace(/\s+/g, "");
  if (clean === "" || !/^[A-Za-z0-9+/]+={0,2}$/.test(clean)) {
    throw new Error(`${what} is not base64`);
  }
  const bytes = Buffer.from(clean, "base64");
  if (bytes.length === 0) throw new Error(`${what} decodes to nothing`);
  return bytes;
}

/**
 * `plugins.updater.pubkey` is the base64 of a minisign public key file: a comment line,
 * then 42 bytes — algorithm, key id, Ed25519 key. This is the key the running app
 * checks a download against, so it is the key this script checks against too.
 */
function readPublicKey(configValue) {
  const text = fromBase64(configValue, "plugins.updater.pubkey").toString("utf8");
  const lines = text.split(/\r?\n/).filter((line) => line.trim() !== "");
  const key = fromBase64(lines[1], "the public key line");
  if (key.length !== MINISIGN_KEY_BYTES) {
    throw new Error(`the public key is ${key.length} bytes, expected ${MINISIGN_KEY_BYTES}`);
  }
  const algorithm = key.subarray(0, 2);
  if (!(algorithm[0] === 0x45 && (algorithm[1] === 0x64 || algorithm[1] === 0x44))) {
    throw new Error("the public key is not an Ed25519 minisign key");
  }
  return {
    comment: lines[0] ?? "",
    keyId: key.subarray(2, 10),
    key: key.subarray(10, MINISIGN_KEY_BYTES),
  };
}

/**
 * A manifest's `signature` (and the contents of a `.sig` file) is the base64 of a
 * minisign signature: an untrusted comment, 74 bytes of algorithm + key id + signature,
 * a trusted comment, and a global signature over the two.
 */
function decodeSignature(value, what) {
  const text = fromBase64(value, what).toString("utf8");
  const lines = text.split(/\r?\n/);
  while (lines.length > 0 && lines[lines.length - 1].trim() === "") lines.pop();
  if (lines.length < 4) throw new Error(`${what} has ${lines.length} lines, expected 4`);

  const blob = fromBase64(lines[1], `${what}: signature`);
  if (blob.length !== MINISIGN_SIG_BYTES) {
    throw new Error(`${what}: signature is ${blob.length} bytes, expected ${MINISIGN_SIG_BYTES}`);
  }
  const trusted = lines[2];
  if (!trusted.startsWith("trusted comment: ")) {
    throw new Error(`${what}: line 3 is not a trusted comment`);
  }
  const global = fromBase64(lines[3], `${what}: global signature`);
  if (global.length !== 64) throw new Error(`${what}: global signature is ${global.length} bytes`);

  const algorithm = blob.subarray(0, 2);
  const prehashed = algorithm[0] === 0x45 && algorithm[1] === 0x44;
  const legacy = algorithm[0] === 0x45 && algorithm[1] === 0x64;
  if (!prehashed && !legacy) throw new Error(`${what}: unknown signature algorithm`);

  return {
    untrustedComment: lines[0],
    prehashed,
    keyId: blob.subarray(2, 10),
    signature: blob.subarray(10, MINISIGN_SIG_BYTES),
    trustedComment: trusted.slice("trusted comment: ".length),
    globalSignature: global,
  };
}

function verifyEd25519(message, signature, key) {
  const spki = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), key]);
  const publicKey = createPublicKey({ key: spki, format: "der", type: "spki" });
  return ed25519Verify(null, message, publicKey, signature);
}

/**
 * The same check `tauri-plugin-updater` makes at runtime (minisign-verify): the key id
 * has to match, the artifact is hashed with BLAKE2b-512 when the signature is prehashed,
 * and the global signature has to cover the signature and the trusted comment.
 */
function verifySignature(bytes, signature, publicKey) {
  if (!publicKey.keyId.equals(signature.keyId)) {
    return {
      ok: false,
      why: `signed with key id ${signature.keyId.toString("hex")}, the config has ${publicKey.keyId.toString("hex")}`,
    };
  }
  const message = signature.prehashed ? createHash("blake2b512").update(bytes).digest() : bytes;
  if (message.length !== (signature.prehashed ? BLAKE2B_512 : bytes.length)) {
    return { ok: false, why: "the BLAKE2b-512 digest came back the wrong size" };
  }
  if (!verifyEd25519(message, signature.signature, publicKey.key)) {
    return { ok: false, why: "the signature does not match the bytes" };
  }
  const global = Buffer.concat([
    signature.signature,
    Buffer.from(signature.trustedComment, "utf8"),
  ]);
  if (!verifyEd25519(global, signature.globalSignature, publicKey.key)) {
    return { ok: false, why: "the global signature does not cover the trusted comment" };
  }
  return { ok: true };
}

/** `timestamp:1700000000\tfile:x.exe\tversion:0.2.0`, when the CLI records it. */
function signedVersion(trustedComment) {
  for (const field of trustedComment.split("\t")) {
    if (field.startsWith("version:")) return field.slice("version:".length);
  }
  return null;
}

// ------------------------------------------------------------------ versions --

/** `v0.2.0` -> `0.2.0`; the tag is the release, the version is what it says. */
function versionOf(tag) {
  return String(tag).replace(/^v/, "");
}

function versionsMatch(a, b) {
  return String(a).replace(/^v/, "") === String(b).replace(/^v/, "");
}

/** `Floaty_0.2.0_x64-setup.exe` -> `0.2.0`, so a misnamed bundle is caught too. */
function versionInName(name) {
  const found = String(name).match(/_(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)[._-]/);
  return found ? found[1] : null;
}

// ------------------------------------------------------------- the GitHub API --

async function apiJson(url, token, what) {
  const response = await fetch(url, {
    headers: {
      accept: "application/vnd.github+json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
  });
  if (!response.ok) {
    throw new Error(`GET ${what} answered ${response.status} ${response.statusText}`);
  }
  return response.json();
}

/**
 * The release for a tag. The list endpoint is used rather than `/releases/tags/<tag>`
 * because a draft release is what this script is normally looking at, and only the list
 * is certain to return one — it is also what tauri-action itself searches.
 */
async function findRelease(api, repo, tag, token) {
  const releases = await apiJson(`${api}/repos/${repo}/releases?per_page=100`, token, `${repo} releases`);
  const found = releases.find((release) => release.tag_name === tag);
  if (!found) {
    throw new Error(
      `no release for ${tag} (looked through ${releases.length} releases: ${releases
        .map((release) => release.tag_name)
        .join(", ") || "none"})`,
    );
  }
  return found;
}

async function downloadAsset(api, repo, asset, token) {
  // the API's asset endpoint is the one that works for a draft release; for a published
  // one an unauthenticated request redirects to the CDN, which is what an installed copy
  // does too
  const url = asset.url || `${api}/repos/${repo}/releases/assets/${asset.id}`;
  const response = await fetch(url, {
    headers: {
      accept: "application/octet-stream",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    redirect: "follow",
  });
  if (!response.ok) {
    throw new Error(`downloading ${asset.name} answered ${response.status} ${response.statusText}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

// ------------------------------------------------------------------ arguments --

function parseArgs(argv) {
  const options = {
    tag: process.env.TAG ?? "",
    repo: process.env.GITHUB_REPOSITORY ?? "",
    artifacts: process.env.ARTIFACTS ?? "",
    manifest: "",
    releaseJson: "",
    requirePlatforms: "",
    config: "src-tauri/tauri.conf.json",
    download: true,
    publish: false,
    dryRun: false,
    help: false,
  };
  const flags = {
    "--tag": "tag",
    "--repo": "repo",
    "--artifacts": "artifacts",
    "--manifest": "manifest",
    "--release-json": "releaseJson",
    "--require-platforms": "requirePlatforms",
    "--config": "config",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      options.help = true;
    } else if (arg === "--no-download") {
      options.download = false;
    } else if (arg === "--publish") {
      options.publish = true;
    } else if (arg === "--dry-run") {
      options.dryRun = true;
    } else if (flags[arg]) {
      const value = argv[index + 1];
      if (value === undefined) throw new Error(`${arg} needs a value`);
      options[flags[arg]] = value;
      index += 1;
    } else {
      throw new Error(`unknown option ${arg} (try --help)`);
    }
  }
  return options;
}

/** tauri-action's `artifactPaths` is a JSON array; a comma-separated list is accepted too. */
function parseArtifacts(value) {
  const text = String(value ?? "").trim();
  if (text === "") return [];
  if (text.startsWith("[")) {
    const parsed = JSON.parse(text);
    return parsed.map((entry) => String(entry));
  }
  return text
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry !== "");
}

const list = (values) => (values.length === 0 ? "none" : values.join(", "));

/** The first bytes of a file, without pulling a whole installer into memory. */
function readHead(path, length = 2) {
  const descriptor = openSync(path, "r");
  try {
    const buffer = Buffer.alloc(length);
    return buffer.subarray(0, readSync(descriptor, buffer, 0, length, 0));
  } finally {
    closeSync(descriptor);
  }
}

// -------------------------------------------------------- the local artifacts --

/**
 * The post-build smoke test. Installing the NSIS installer on the runner and watching a
 * GUI app come up is not something this can do reliably (see the workflow), so it checks
 * what does work everywhere: the file is there, it is not empty, it is the file it claims
 * to be, and — the part that matters for the updater — the signature the release will
 * carry verifies over its bytes against the key in tauri.conf.json.
 */
function checkArtifacts(paths, publicKey, version) {
  section(`built artifacts (${paths.length})`);
  if (paths.length === 0) {
    bad("no artifacts to smoke-test");
    return;
  }

  const files = [];
  const signatures = [];
  for (const path of paths) {
    if (!existsSync(path)) {
      bad(`artifact ${basename(path)}`, "missing");
      continue;
    }
    const size = statSync(path).size;
    if (size === 0) {
      bad(`artifact ${basename(path)}`, "is zero bytes");
      continue;
    }
    (path.endsWith(".sig") ? signatures : files).push({ path, size });
  }

  for (const file of files) {
    const head = readHead(file.path);
    const isExe = file.path.endsWith(".exe");
    const isZip = file.path.endsWith(".zip");
    const looksRight = isExe ? head[0] === 0x4d && head[1] === 0x5a : isZip ? head[0] === 0x50 && head[1] === 0x4b : true;
    const kind = isExe ? "MZ" : isZip ? "PK" : "no header check";
    if (looksRight) {
      ok(`artifact ${basename(file.path)}`, `${file.size} bytes, ${kind}`);
    } else {
      bad(`artifact ${basename(file.path)}`, `${file.size} bytes but the ${isExe ? "MZ" : "PK"} header is missing`);
    }
    const named = versionInName(basename(file.path));
    if (named !== null && !versionsMatch(named, version)) {
      bad(`artifact ${basename(file.path)}`, `names version ${named}, the tag says ${version}`);
    }
  }

  if (signatures.length === 0) {
    bad(
      "no updater signature was produced",
      "no .sig among the artifacts — bundle.createUpdaterArtifacts has to be on for the feed to be signable",
    );
  }

  for (const signature of signatures) {
    const target = signature.path.slice(0, -".sig".length);
    if (!existsSync(target)) {
      bad(`signature ${basename(signature.path)}`, `there is no ${basename(target)} beside it`);
      continue;
    }
    const bytes = readFileSync(target);
    try {
      const decoded = decodeSignature(readFileSync(signature.path, "utf8"), basename(signature.path));
      const result = verifySignature(bytes, decoded, publicKey);
      if (result.ok) {
        ok(
          `signature ${basename(signature.path)}`,
          `${decoded.prehashed ? "prehashed" : "legacy"} ${signature.size} bytes, key id ${decoded.keyId.toString("hex")}, verifies over ${basename(target)} (${bytes.length} bytes)`,
        );
        note(`  trusted comment`, decoded.trustedComment);
      } else {
        bad(`signature ${basename(signature.path)}`, result.why);
      }
    } catch (error) {
      bad(`signature ${basename(signature.path)}`, error.message);
    }
  }
}

// ------------------------------------------------------------- the feed itself --

/**
 * Where a manifest entry points. tauri-action writes the asset's browser download URL
 * (`https://github.com/<owner>/<repo>/releases/download/<tag>/<name>`); the API asset URL
 * is accepted too, and the tag in a download URL has to be the tag being published —
 * an entry left over from an older release is exactly the thing to refuse.
 */
function readEntryUrl(url, repo, tag) {
  if (typeof url !== "string" || url === "") return { why: "the url is empty" };
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return { why: `${url} is not a URL` };
  }
  const [owner, name] = String(repo ?? "").split("/");
  const named = Boolean(owner && name);
  const path = decodeURIComponent(parsed.pathname);
  const assetMatch = path.match(/^\/repos\/([^/]+)\/([^/]+)\/releases\/assets\/(\d+)$/);
  if (assetMatch) {
    if (named && (assetMatch[1] !== owner || assetMatch[2] !== name)) {
      return { why: `${url} belongs to ${assetMatch[1]}/${assetMatch[2]}, not ${repo}` };
    }
    return { assetId: Number(assetMatch[3]) };
  }
  const downloadMatch = path.match(/^\/([^/]+)\/([^/]+)\/releases\/download\/([^/]+)\/(.+)$/);
  if (downloadMatch) {
    if (named && (downloadMatch[1] !== owner || downloadMatch[2] !== name)) {
      return { why: `${url} belongs to ${downloadMatch[1]}/${downloadMatch[2]}, not ${repo}` };
    }
    if (!versionsMatch(downloadMatch[3], tag)) {
      return { why: `${url} is a download for ${downloadMatch[3]}, not for ${tag}` };
    }
    return { name: basename(downloadMatch[4]) };
  }
  return { why: `${url} is not a release asset of ${named ? repo : "a GitHub release"}` };
}

/** `owner/name`, from whatever the release payload happens to carry. */
function repoOf(release) {
  const candidates = [release?.url, release?.html_url, ...(release?.assets ?? []).map((asset) => asset.url)];
  for (const candidate of candidates) {
    const text = String(candidate ?? "");
    const found =
      text.match(/\/repos\/([^/]+)\/([^/]+)\//) ?? text.match(/github\.com\/([^/]+)\/([^/]+)\/releases\//);
    if (found) return `${found[1]}/${found[2]}`;
  }
  return "";
}

function findAsset(release, entry) {
  const assets = Array.isArray(release.assets) ? release.assets : [];
  if (entry.assetId !== undefined) return assets.find((asset) => asset.id === entry.assetId) ?? null;
  return assets.find((asset) => asset.name === entry.name) ?? null;
}

async function checkFeed({ release, manifest, repo, tag, publicKey, options, token, api }) {
  const version = versionOf(tag);

  section("the manifest");
  if (manifest.version === undefined) {
    bad("the manifest has no version");
  } else if (versionsMatch(manifest.version, version)) {
    ok("manifest version", `${manifest.version} equals the tag ${tag}`);
  } else {
    bad("manifest version", `${manifest.version} is not the version of ${tag} (${version})`);
  }

  const platforms = manifest.platforms ?? {};
  const keys = Object.keys(platforms);
  if (keys.length === 0) bad("the manifest lists no platforms");
  else ok("manifest platforms", list(keys));

  const required = String(options.requirePlatforms ?? "")
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry !== "");
  for (const platform of required) {
    if (keys.includes(platform)) ok(`platform ${platform}`, "present");
    else bad(`platform ${platform}`, `missing — the manifest has ${list(keys)}`);
  }

  if (!release) {
    note("release assets", "not checked (no release was read)");
    return;
  }

  const assets = Array.isArray(release.assets) ? release.assets : [];
  section(`release ${tag} (id ${release.id}, ${release.draft ? "draft" : "published"}, ${assets.length} assets)`);
  for (const asset of assets) {
    note(`  asset ${asset.name}`, `${asset.size} bytes`);
  }
  if (assets.some((asset) => asset.size === 0)) {
    bad("an asset on the release is zero bytes", assets.filter((asset) => asset.size === 0).map((asset) => asset.name).join(", "));
  }

  const downloaded = new Map();
  for (const [platform, entry] of Object.entries(platforms)) {
    section(`platform ${platform}`);
    const where = readEntryUrl(entry?.url, repo, tag);
    if (where.why) {
      bad(`${platform}: url`, where.why);
      continue;
    }
    const asset = findAsset(release, where);
    if (!asset) {
      bad(`${platform}: artifact`, `${entry.url} is not an asset of release ${release.id}`);
      continue;
    }
    ok(`${platform}: artifact`, `${asset.name}, ${asset.size} bytes`);
    if (asset.size === 0) bad(`${platform}: artifact`, `${asset.name} is zero bytes`);

    const signatureAsset = release.assets.find((candidate) => candidate.name === `${asset.name}.sig`);
    if (!signatureAsset) {
      bad(`${platform}: signature file`, `${asset.name}.sig is not on the release`);
    } else if (signatureAsset.size === 0) {
      bad(`${platform}: signature file`, `${asset.name}.sig is zero bytes`);
    } else {
      ok(`${platform}: signature file`, `${signatureAsset.name}, ${signatureAsset.size} bytes`);
    }

    if (typeof entry?.signature !== "string" || entry.signature === "") {
      bad(`${platform}: signature`, "the manifest carries no signature for this platform");
      continue;
    }

    let decoded;
    try {
      decoded = decodeSignature(entry.signature, `${platform} signature`);
    } catch (error) {
      bad(`${platform}: signature`, error.message);
      continue;
    }
    ok(
      `${platform}: signature`,
      `${decoded.prehashed ? "prehashed" : "legacy"} key id ${decoded.keyId.toString("hex")}`,
    );
    const stamped = signedVersion(decoded.trustedComment);
    if (stamped !== null && !versionsMatch(stamped, version)) {
      bad(`${platform}: signed version`, `the signature is for ${stamped}, the tag says ${version}`);
    } else if (stamped !== null) {
      ok(`${platform}: signed version`, stamped);
    } else {
      note(`${platform}: signed version`, "the trusted comment carries none (signed by an older CLI)");
    }

    if (signatureAsset && signatureAsset.size > 0) {
      try {
        const onTheRelease = (await downloadAsset(api, repo, signatureAsset, token)).toString("utf8").replace(/\s+/g, "");
        const inTheManifest = String(entry.signature).replace(/\s+/g, "");
        if (onTheRelease === inTheManifest) ok(`${platform}: signature matches the .sig on the release`);
        else bad(`${platform}: signature`, `${signatureAsset.name} and the manifest's signature differ`);
      } catch (error) {
        bad(`${platform}: signature file`, error.message);
      }
    }

    // the bytes the signature is checked over: what the release serves, or — with
    // --no-download — the file the build produced
    let bytes = null;
    if (options.download) {
      try {
        if (!downloaded.has(asset.name)) {
          downloaded.set(asset.name, await downloadAsset(api, repo, asset, token));
        }
        bytes = downloaded.get(asset.name);
        if (bytes.length !== asset.size) {
          bad(`${platform}: artifact`, `${asset.name} downloaded as ${bytes.length} bytes, the release says ${asset.size}`);
        }
      } catch (error) {
        bad(`${platform}: artifact`, error.message);
      }
    } else {
      const local = options.artifacts.find((path) => basename(path) === asset.name);
      if (!local) {
        bad(`${platform}: artifact`, `--no-download and no local ${asset.name} to verify`);
      } else {
        bytes = readFileSync(local);
        note(`${platform}: artifact`, `verifying the local ${local}`);
      }
    }

    if (bytes === null) continue;
    if (bytes.length === 0) {
      bad(`${platform}: artifact`, "zero bytes");
      continue;
    }
    const result = verifySignature(bytes, decoded, publicKey);
    if (result.ok) {
      ok(`${platform}: signature verifies`, `over ${bytes.length} bytes against the key in the config`);
    } else {
      bad(`${platform}: signature verifies`, result.why);
    }
  }
}

// ------------------------------------------------------------------- publishing --

/**
 * Takes the release out of draft. Only ever called once every check above has passed —
 * the guards are pointless if something else can publish the release anyway.
 */
async function publish({ api, repo, release, token, dryRun }) {
  section(`publishing ${release.tag_name}`);
  if (release.draft === false) {
    note("already published", release.html_url);
    return;
  }
  if (dryRun) {
    note("dry run", `would publish release ${release.id} (${release.html_url})`);
    return;
  }
  const response = await fetch(`${api}/repos/${repo}/releases/${release.id}`, {
    method: "PATCH",
    headers: {
      accept: "application/vnd.github+json",
      "content-type": "application/json",
      authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({ draft: false }),
  });
  if (!response.ok) {
    throw new Error(`publishing release ${release.id} answered ${response.status} ${response.statusText}`);
  }
  const published = await response.json();
  ok("release published", `${published.html_url} (draft: ${published.draft})`);
}

// ----------------------------------------------------------------------- main --

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    console.log(readFileSync(new URL(import.meta.url), "utf8").split("*/")[0].replace(/^#![^\n]*\n/, ""));
    return 0;
  }

  const api = process.env.GITHUB_API_URL || "https://api.github.com";
  const token = process.env.GITHUB_TOKEN || process.env.GH_TOKEN || "";
  const tag = options.tag;
  if (!tag) throw new Error("--tag (or $TAG) is required: the tag is the release");
  options.artifacts = parseArtifacts(options.artifacts);

  console.log("verify-updater-feed");
  console.log(`  tag          ${tag}`);
  console.log(`  version      ${versionOf(tag)}`);
  console.log(`  repository   ${options.repo || "(none — the release is not read)"}`);
  console.log(`  config       ${resolve(options.config)}`);
  console.log(`  artifacts    ${list(options.artifacts)}`);
  console.log(`  token        ${token ? "present" : "absent (drafts and private assets will not be readable)"}`);

  section("the configuration");
  const config = JSON.parse(readFileSync(options.config, "utf8"));
  if (config.version === undefined) {
    bad("tauri.conf.json has no version");
  } else if (versionsMatch(config.version, tag)) {
    ok("tauri.conf.json version", `${config.version} equals the tag ${tag}`);
  } else {
    bad("tauri.conf.json version", `${config.version} is not the version of ${tag} (${versionOf(tag)})`);
  }

  const updater = config.plugins?.updater ?? {};
  const endpoints = Array.isArray(updater.endpoints) ? updater.endpoints : [];
  if (endpoints.length > 0) ok("updater endpoint", list(endpoints));
  else bad("plugins.updater.endpoints", "empty — an installed copy would have no address to check");

  // ...and it has to be an address that always exists. The escape hatch for a bad release
  // is to withdraw it and publish a fixed one, which a per-release URL could not survive,
  // and there is no server that could be kept alive in its place (ADR 0002).
  for (const endpoint of endpoints) {
    if (String(endpoint).includes("/releases/latest/download/latest.json")) {
      ok("updater endpoint is the stable one", String(endpoint));
    } else {
      bad(
        "updater endpoint",
        `${endpoint} is not a .../releases/latest/download/latest.json address — withdrawing a release is the escape hatch, so the address has to be one that always exists`,
      );
    }
  }

  let publicKey = null;
  try {
    publicKey = readPublicKey(updater.pubkey);
    ok("updater public key", `key id ${publicKey.keyId.toString("hex")}, ${publicKey.comment}`);
  } catch (error) {
    bad("plugins.updater.pubkey", error.message);
  }

  if (options.artifacts.length > 0 && publicKey) {
    checkArtifacts(options.artifacts, publicKey, versionOf(tag));
  }

  let release = null;
  if (options.releaseJson) {
    release = JSON.parse(readFileSync(options.releaseJson, "utf8"));
  } else if (options.repo && !options.manifest) {
    try {
      release = await findRelease(api, options.repo, tag, token);
    } catch (error) {
      bad("the release", error.message);
    }
  }

  let manifest = null;
  if (options.manifest) {
    section("the manifest on disk");
    manifest = JSON.parse(readFileSync(options.manifest, "utf8"));
    note("read", options.manifest);
  } else if (release) {
    const asset = (release.assets ?? []).find((candidate) => candidate.name === "latest.json");
    if (!asset) {
      bad("latest.json", `release ${release.id} has no latest.json asset`);
    } else if (asset.size === 0) {
      bad("latest.json", "the asset is zero bytes");
    } else {
      try {
        const bytes = await downloadAsset(api, options.repo, asset, token);
        manifest = JSON.parse(bytes.toString("utf8"));
        note("latest.json", `${asset.size} bytes from release ${release.id}`);
      } catch (error) {
        bad("latest.json", error.message);
      }
    }
  }

  if (manifest && publicKey) {
    // the repository a feed entry has to belong to: what was asked for, or what the
    // release payload itself says (a release read from a file has no --repo)
    const repo = options.repo || repoOf(release);
    if (repo && repo !== options.repo) note("repository", `${repo} (from the release payload)`);
    await checkFeed({ release, manifest, repo, tag, publicKey, options, token, api });
  } else if (manifest && !publicKey) {
    note("the feed", "not checked: the public key could not be read");
  }

  if (options.publish) {
    if (failures > 0) {
      bad("not publishing", `${failures} check(s) failed`);
    } else if (!release) {
      bad("not publishing", "no release was read");
    } else if (options.releaseJson && !options.dryRun) {
      // publishing is the one thing here that writes, so it only ever acts on a release
      // read from the API — never on one handed in as a file
      bad("not publishing", "--publish needs the release read from the API, not --release-json");
    } else {
      await publish({ api, repo: options.repo || repoOf(release), release, token, dryRun: options.dryRun });
    }
  }

  console.log(`\n${checks} checks, ${failures} failed`);
  return failures === 0 ? 0 : 1;
}

// `process.exit()` here would tear the loop down while undici is still closing its
// sockets, which trips a libuv assertion on Windows (exit 127); setting the code and
// letting the loop drain exits cleanly with the same status.
main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((error) => {
    console.error(`::error::verify-updater-feed: ${error.message}`);
    console.error(`verify-updater-feed: ${error.message}`);
    process.exitCode = 2;
  });
