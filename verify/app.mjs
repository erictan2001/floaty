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
 * The world a probe stands in — which overlay pages are open and whether they are
 * *alive*, the console errors, the exit ladder — is `runtime.mjs`; what is left here
 * is what health says about the desktop, which no unit test can say for us.
 *
 * This talks to the real store through the real IPC, so it is the only check here
 * that proves the app is well — a green unit test suite says nothing about whether
 * the desktop drew. It also means a probe can change the user's desktop: prefer
 * read-only commands, and clean up what you make.
 */

import path from "node:path";
import { fileURLToPath } from "node:url";
import { APP_PORT, Session, listTargets } from "./cdp.mjs";
import { attach, probe } from "./runtime.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
/**
 * A flag and its value, or nothing when the flag is absent.
 *
 * Written as an explicit index set rather than `i !== flag + 1`: with no `--port`
 * on the command line `indexOf` returns -1, so `flag + 1` is 0 — and the filter
 * then ate the command itself (`verify:app js "…"` reported the js *expression* as
 * an unknown command).
 *
 * Not the runtime's `flag()`: that one reads a value, and this has to hand back
 * positions too, so a flag can be taken back out of the argument list.
 */
function flagValue(flag) {
  const at = args.indexOf(flag);
  return at < 0 ? [] : [at, at + 1];
}
const portAt = flagValue("--port");
const pageAt = flagValue("--page");
const port = portAt.length ? Number(args[portAt[1]]) : APP_PORT;
/** Which page to talk to: the overlay by default, `#/palette` or any other hash. */
const page = pageAt.length ? args[pageAt[1]] : "#/overlay";
const nthAt = flagValue("--nth");
/** Which match, when several windows share a page: one overlay per screen. */
const nth = nthAt.length ? Number(args[nthAt[1]]) : 0;
const skip = new Set([...portAt, ...pageAt, ...nthAt]);
// a bare `--` is how an argument is passed through npm, and it is not a command
const rest = args.filter((a, i) => a !== "--" && !skip.has(i));
const command = rest[0] ?? "health";

const { problems, skip: cannot, finish } = probe("verify:app");

/**
 * How long an app that is *already running* gets to answer.
 *
 * Not the runtime's 60s: the app here is supposed to be up, so the only thing worth
 * waiting for is a page caught mid-reload — and a minute is a long time to sit on a
 * debug port nobody is serving. It does have to cover a *cold* dev server, though: the
 * first `src/bootstrap.ts` after a restart is 45s on an idle machine and has been
 * measured at 86s while the machine was busy (a game, a second dev tree), and the page
 * has no Tauri bridge until its document loads at all.
 */
const UP_TIMEOUT = 180;

/**
 * One overlay window's health.
 *
 * There is **one overlay per screen** (see `spawn_overlay_windows`), so a window is
 * healthy when it mounts exactly the records that fall on its own screen — not when it
 * mounts all of them. `shouldMount` is that count, computed the way the window does it.
 */
const WINDOW_HEALTH = `(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const area = await invoke("floaty_overlay_area");
  const records = await invoke("floaty_list");
  const screen = area ? area.screen : null;
  const mine = screen
    ? records.filter((r) =>
        r.x >= screen.logical.x && r.x < screen.logical.x + screen.logical.w &&
        r.y >= screen.logical.y && r.y < screen.logical.y + screen.logical.h)
    : records;
  const slots = [...document.querySelectorAll("#desktop-canvas > *")];
  const ids = new Set(slots.map((el) => (el.id || "").replace(/^slot-/, "")));
  const tiles = [...document.querySelectorAll("img.tile-icon")];
  const folderImgs = [...document.querySelectorAll(".ftile img, .fmini img")];
  const failed = [...tiles, ...folderImgs].filter((i) => i.complete && i.naturalWidth === 0);
  return {
    screen: screen ? screen.name : "(no area reported)",
    scale: screen ? screen.scale : 0,
    dpr: window.devicePixelRatio,
    total: records.length,
    shouldMount: mine.length,
    mounted: slots.length,
    missing: mine.filter((r) => !ids.has(r.id)).map((r) => r.id),
    strangers: [...ids].filter((id) => !mine.some((r) => r.id === id)),
    tileIcons: tiles.length,
    iconsFailed: failed.length,
    iconSizes: [...new Set([...tiles, ...folderImgs].filter((i) => i.naturalWidth > 0)
      .map((i) => i.naturalWidth + "x" + i.naturalHeight))].sort(),
    hidden: document.visibilityState,
  };
})()`;

/** One `WINDOW_HEALTH` report per overlay page. */
async function readWindows(app) {
  const reports = [];
  for (let nth = 0; nth < app.sessions.length; nth += 1) {
    reports.push(await app.page(nth).evaluate(WINDOW_HEALTH));
  }
  return reports;
}

/**
 * Every overlay window, checked as a window rather than as "the desktop".
 *
 * With two screens there are two overlays, and the interesting failure is not one of
 * them being wrong on its own — it is a record that neither window drew, or one that
 * both drew. So the sum is checked against the record list as well.
 */
async function health(app) {
  // A cold dev server serves `src/bootstrap.ts` in ~45s on an idle machine and 86s on a
  // busy one, so a *live* page can still be an empty one for minutes: sampling once
  // reported "0 mounted, 44 belong here" on an app that was perfectly healthy. Wait for
  // the mount to finish, then check what it drew — the wait is a predicate with a
  // deadline, so a desktop that is genuinely empty settles immediately.
  const settled = await app
    .until(
      "every overlay to finish mounting",
      async () => {
        const reports = await readWindows(app);
        const mounted = reports.reduce((n, r) => n + r.mounted, 0);
        const want = reports.reduce((n, r) => n + r.shouldMount, 0);
        return mounted === want ? reports : null;
      },
      { timeout: 240 },
    )
    .catch(() => null);
  const windows = settled ?? (await readWindows(app));
  const screens = [];
  let mountedTotal = 0;
  let expectedTotal = 0;
  let total = 0;

  for (const report of windows) {
    mountedTotal += report.mounted;
    expectedTotal += report.shouldMount;
    total = Math.max(total, report.total);

    const where = `${report.screen} @${report.scale}x (dpr ${report.dpr})`;
    screens.push(where);
    if (report.mounted !== report.shouldMount)
      problems.push(`${where}: ${report.mounted} mounted, ${report.shouldMount} belong here`);
    if (report.missing.length)
      problems.push(`${where}: no slot for ${report.missing.join(", ")}`);
    if (report.strangers.length)
      problems.push(`${where}: drew a floatie that is not on this screen: ${report.strangers.join(", ")}`);
    if (report.iconsFailed) problems.push(`${where}: ${report.iconsFailed} icon(s) failed to load`);
    if (report.hidden !== "visible")
      problems.push(`${where}: the page believes it is ${report.hidden} — timers will be throttled`);
  }
  // Drained once, across every page, and not per window: `takeErrors()` sweeps all of
  // them, so draining inside the loop would file every screen's complaints under the
  // first. What is left is the app's own text, and the screens are named so a reader
  // knows which windows were asked.
  for (const error of app.takeErrors()) problems.push(`${screens.join(" or ")}: ${error}`);

  // Between the windows: every record is drawn exactly once, wherever it lives.
  if (mountedTotal !== expectedTotal)
    problems.push(`${mountedTotal} floaties drawn across ${app.sessions.length} window(s) for ${expectedTotal} that belong to a screen`);
  if (expectedTotal !== total)
    problems.push(`${total - expectedTotal} floatie(s) are on no screen at all — run the Diagnostics tab's "bring back off-screen floaties"`);
  // An empty desktop and an app that has lost its store look the same from here: no records
  // to mount, none drawn, nothing to disagree about. `total` is the app's own count of what
  // it knows, so zero of them means the store did not load — and a probe that calls that
  // healthy has green-checked a broken install.
  if (!total) problems.push("the app knows about no floaties at all — the store did not load");

  console.log(JSON.stringify({ windows, records: total, drawn: mountedTotal }, null, 2));
  if (problems.length === 0) {
    console.log(`\n${app.sessions.length} overlay(s) healthy — ${mountedTotal} floatie(s) drawn, once each`);
    return;
  }
  console.log("");
  for (const problem of problems) console.log(`PROBLEM: ${problem}`);
}

/** The live overlay pages of a running app, or a probe that could not run. */
const overlayPages = () =>
  attach({ port, screens: 1, timeout: UP_TIMEOUT }).catch((error) => cannot(error.message));

async function main() {
  if (command === "health") {
    const app = await overlayPages();
    try {
      await health(app);
    } finally {
      await app.close();
    }
    return;
  }
  if (command === "reload") {
    // Frontend changes reach a running page only through Vite's HMR, and a long-lived
    // overlay window stops applying it: without this, a probe can measure yesterday's
    // code and report the fix as not working.
    const app = await overlayPages();
    try {
      for (let nth = 0; nth < app.sessions.length; nth += 1) {
        await app.page(nth).send("Page.reload", { ignoreCache: true });
      }
      console.log(`reloaded ${app.sessions.length} page(s)`);
    } finally {
      await app.close();
    }
    return;
  }

  // `--page` can name a page the runtime knows nothing about — `#/palette` is a second
  // window, built the first time something asks for it — so these open the page
  // themselves. They are a way in, not a check: nothing waits for a bridge here, and a
  // corpse answers `js` exactly as readily as a wrong app does.
  const session = await Session.open(port, rest[0] === "health" ? "#/overlay" : page, nth);
  try {
    if (command === "js") {
      const expression = rest[1];
      if (!expression) throw new Error("verify:app js <expression>");
      console.log(JSON.stringify(await session.evaluate(expression), null, 2));
      return;
    }
    if (command === "key") {
      const combo = rest[1];
      if (!combo) throw new Error("verify:app key <combo>, e.g. ctrl+z");
      await session.key(combo);
      console.log(`sent ${combo}`);
      return;
    }
    if (command === "shot") {
      const file = path.resolve(here, rest[1] ?? "out/app.png");
      await session.screenshot(file);
      console.log(file);
      return;
    }
    if (command === "targets") {
      for (const t of await listTargets(port)) console.log(`${t.type}  ${t.url}`);
      return;
    }
    throw new Error(`verify:app <health|js|key|shot|targets|reload> (got "${command}")`);
  } finally {
    await session.close();
  }
}

try {
  await main();
} catch (err) {
  // No app on the debug port (or no port) is not the same as an app that is up and
  // wrong: that is exit 2, and exit 1 stays for "the app failed a check".
  cannot(`could not run — ${err.message}`);
}
// Everything was closed on the way out of `main`, sockets included, so the process can
// leave now with the code the checks mean rather than one that depends on the timing.
await finish();
