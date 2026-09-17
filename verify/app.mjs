/**
 * Check a *running* floaty over its debug port.
 *
 * The app has to be started with the port open, because the overlay is a real
 * WebView2 window and there is no other way in:
 *
 *     $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
 *     npm run tauri dev
 *
 * then:
 *
 *     npm run verify:app                 # health: mounted, icons, console errors
 *     npm run verify:app -- js "await window.__TAURI_INTERNALS__.invoke('floaty_undo_state')"
 *     npm run verify:app -- key ctrl+z   # a real key event
 *     npm run verify:app -- shot out/overlay.png
 *
 * This talks to the real store through the real IPC, so it is the only check here
 * that proves the app is well — a green unit test suite says nothing about whether
 * the desktop drew. It also means a probe can change the user's desktop: prefer
 * read-only commands, and clean up what you make.
 */

import path from "node:path";
import { fileURLToPath } from "node:url";
import { Session, listTargets, realErrors } from "./cdp.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
/**
 * A flag and its value, or nothing when the flag is absent.
 *
 * Written as an explicit index set rather than `i !== flag + 1`: with no `--port`
 * on the command line `indexOf` returns -1, so `flag + 1` is 0 — and the filter
 * then ate the command itself (`verify:app js "…"` reported the js *expression* as
 * an unknown command).
 */
function flagValue(flag) {
  const at = args.indexOf(flag);
  return at < 0 ? [] : [at, at + 1];
}
const portAt = flagValue("--port");
const pageAt = flagValue("--page");
const port = portAt.length ? Number(args[portAt[1]]) : 9222;
/** Which page to talk to: the overlay by default, `#/palette` or any other hash. */
const page = pageAt.length ? args[pageAt[1]] : "#/overlay";
const skip = new Set([...portAt, ...pageAt]);
// a bare `--` is how an argument is passed through npm, and it is not a command
const rest = args.filter((a, i) => a !== "--" && !skip.has(i));
const command = rest[0] ?? "health";

const HEALTH = `(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const records = await invoke("floaty_list");
  const slots = [...document.querySelectorAll("#desktop-canvas > *")];
  const ids = new Set(slots.map((el) => (el.id || "").replace(/^slot-/, "")));
  const tiles = [...document.querySelectorAll("img.tile-icon")];
  const folderImgs = [...document.querySelectorAll(".ftile img, .fmini img")];
  const failed = [...tiles, ...folderImgs].filter((i) => i.complete && i.naturalWidth === 0);
  return {
    records: records.length,
    mounted: slots.length,
    missingSlots: records.filter((r) => !ids.has(r.id)).map((r) => r.id),
    tileIcons: tiles.length,
    folderIcons: folderImgs.length,
    iconsFailed: failed.length,
    iconSizes: [...new Set([...tiles, ...folderImgs].filter((i) => i.naturalWidth > 0)
      .map((i) => i.naturalWidth + "x" + i.naturalHeight))].sort(),
    hidden: document.visibilityState,
  };
})()`;

async function health() {
  const session = await Session.open(port, "#/overlay");
  const report = await session.evaluate(HEALTH);
  const errors = realErrors(session.errors);
  await session.close();

  const problems = [];
  if (report.records !== report.mounted)
    problems.push(`${report.mounted} mounted for ${report.records} records`);
  if (report.missingSlots.length)
    problems.push(`no slot for: ${report.missingSlots.join(", ")}`);
  if (report.iconsFailed) problems.push(`${report.iconsFailed} icon(s) failed to load`);
  if (report.hidden !== "visible")
    problems.push(`the page believes it is ${report.hidden} — timers will be throttled`);
  for (const error of errors) problems.push(error);

  console.log(JSON.stringify(report, null, 2));
  if (problems.length === 0) {
    console.log("\noverlay healthy");
    return 0;
  }
  console.log("");
  for (const problem of problems) console.log(`PROBLEM: ${problem}`);
  return 1;
}

async function main() {
  if (command === "health") return await health();

  const session = await Session.open(port, rest[0] === "health" ? "#/overlay" : page);
  try {
    if (command === "js") {
      const expression = rest[1];
      if (!expression) throw new Error("verify:app js <expression>");
      console.log(JSON.stringify(await session.evaluate(expression), null, 2));
      return 0;
    }
    if (command === "key") {
      const combo = rest[1];
      if (!combo) throw new Error("verify:app key <combo>, e.g. ctrl+z");
      await session.key(combo);
      console.log(`sent ${combo}`);
      return 0;
    }
    if (command === "shot") {
      const file = path.resolve(here, rest[1] ?? "out/app.png");
      await session.screenshot(file);
      console.log(file);
      return 0;
    }
    if (command === "targets") {
      for (const t of await listTargets(port)) console.log(`${t.type}  ${t.url}`);
      return 0;
    }
    throw new Error(`verify:app <health|js|key|shot|targets> (got "${command}")`);
  } finally {
    await session.close();
  }
}

// `process.exitCode` and not `process.exit()`: the socket has to be allowed to
// finish closing, and a run's exit code must not depend on how fast that is.
try {
  process.exitCode = await main();
} catch (err) {
  // No app on the debug port (or no port) is not the same as an app that is up and
  // wrong: that is exit 2, and exit 1 stays for "the app failed a check".
  console.error(`verify:app could not run — ${err.message}`);
  process.exitCode = 2;
}
