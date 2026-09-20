/**
 * A fullscreen app must take only its own screen away.
 *
 * The rule is per screen: a fullscreen window in front of one monitor hides that monitor's
 * desktop layer and leaves every other one exactly as it was. It used to be one global
 * answer, so a fullscreen app anywhere hid every overlay — the icons on the other screen
 * disappeared until the desktop was clicked.
 *
 * This opens a real borderless window over a chosen screen, reads the presence state back
 * from the app, and closes it again. The window is DPI-aware on purpose: without that the
 * coordinates are virtualised (a 2880x1920 screen reports 1440x960) and the window does not
 * actually cover the screen, so the rule correctly decides nothing and the probe would pass
 * against the broken build.
 *
 * It does put a window on one screen for a couple of seconds. Usage:
 *   node verify/presence.mjs [--keep-open]
 */
import { spawn, spawnSync } from "node:child_process";
import { readFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { APP_PORT, Session } from "./cdp.mjs";

const keepOpen = process.argv.includes("--keep-open");
// Beside this file, not in a temp folder: a check that breaks when Temp is cleaned is not a
// check. The script is ASCII-only on purpose — PowerShell parses a BOM-less non-ASCII file
// in the console codepage and dies on the first dash it does not recognise.
const PROBE = fileURLToPath(new URL("./fullscreen-probe.ps1", import.meta.url));

const session = await Session.open(APP_PORT, "#/overlay", 0);
const presence = () =>
  session.evaluate(`return JSON.stringify(await window.__TAURI_INTERNALS__.invoke("floaty_presence"));`).then(JSON.parse);
const windows = async () => {
  const report = await session.evaluate(
    `return JSON.stringify((await window.__TAURI_INTERNALS__.invoke("floaty_diagnostics")).windows ?? []);`,
  ).then(JSON.parse);
  return Object.fromEntries(report.map((w) => [w.label, w.visible]));
};

/** Every desktop layer, and whether it is hidden — the whole point of the check. */
const hidden = async () => {
  const report = await presence();
  const layers = report.screens ?? [];
  return {
    byLabel: Object.fromEntries(layers.map((s) => [s.label, s.hidden])),
    byScreen: Object.fromEntries(layers.map((s) => [s.screen, s.hidden])),
    machineHidden: report.hidden,
    machineQuiet: report.quiet,
    foreground: report.foreground,
  };
};

const openFullscreen = (index) => {
  // Keep the probe's own output: it says whether the window actually became foreground, and
  // a probe that covers the screen without being in front tests nothing.
  // Through `start`, so the probe gets a console of its own: spawned straight from here with
  // a pipe, PowerShell exits immediately and no window is ever created.
  const status = `${process.env.TEMP}\\floaty-probe\\probe-status.txt`;
  try {
    rmSync(status, { force: true });
  } catch {
    /* nothing to remove */
  }
  const child = spawn(
    "cmd",
    ["/c", "start", "", "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", PROBE, String(index)],
    { detached: true, stdio: "ignore" },
  );
  child.on("error", (err) => console.error(`  probe could not start: ${String(err)}`));
  child.unref();
  return { child, status };
};

/**
 * Close the probe by the pid it reported.
 *
 * `cmd /c start` gives it a console of its own, so its pid is not ours to kill, and its
 * "main window" is that console — matching on the form's title finds nothing.
 */
const closeProbe = (status) => {
  let pid = null;
  try {
    pid = Number(/pid=(\d+)/.exec(readFileSync(status, "utf8"))?.[1] ?? "");
  } catch {
    /* no status file: nothing to close by pid */
  }
  if (!pid) return false;
  spawnSync("powershell", ["-NoProfile", "-Command", `Stop-Process -Id ${pid} -Force -ErrorAction SilentlyContinue`], {
    stdio: "ignore",
  });
  return true;
};

const waitFor = async (want, what, tries = 24) => {
  for (let i = 0; i < tries; i += 1) {
    const state = await hidden();
    if (want(state)) return state;
    await new Promise((r) => setTimeout(r, 250));
  }
  return null;
};

const problems = [];
const before = await hidden();
const layers = Object.keys(before.byLabel);
console.log(`presence: ${layers.length} desktop layer(s): ${layers.join(", ")}`);
if (layers.length < 2) {
  console.error("presence: needs two desktop layers — this desktop has one screen, or the app is not running");
  process.exit(2);
}
if (before.machineHidden) problems.push(`the desktop was already hidden before the probe: ${before.foreground}`);

// One screen at a time, in the order the app labels them.
for (const [index, label] of layers.entries()) {
  const screen = (await presence()).screens.find((s) => s.label === label)?.screen;
  const probe = openFullscreen(index);
  const state = await waitFor((s) => s.byLabel[label] === true, `fullscreen on ${label}`);
  try {
    console.log(`  probe: ${readFileSync(probe.status, "utf8").trim()}`);
  } catch {
    console.log("  probe: no status file — the window did not open");
  }
  if (!state) {
    problems.push(`a fullscreen window on ${label} (${screen}) did not hide it — ${JSON.stringify(await hidden())}`);
  } else {
    const others = Object.entries(state.byLabel).filter(([l]) => l !== label);
    const leaked = others.filter(([, isHidden]) => isHidden).map(([l]) => l);
    if (leaked.length) {
      problems.push(
        `a fullscreen window on ${label} also hid ${leaked.join(", ")} — every screen decides for itself`,
      );
    }
    if (state.machineQuiet) {
      problems.push(
        `a fullscreen window on ${label} made the whole machine quiet — the other screen's audio must keep running`,
      );
    }
    // The state is stored before the window is hidden, so a reader can see "hidden" a poll
    // before the hide lands. Wait for the fact this is asserting rather than sampling once.
    let visible = await windows();
    for (let i = 0; i < 8 && visible[label] !== false; i += 1) {
      await new Promise((r) => setTimeout(r, 250));
      visible = await windows();
    }
    if (visible[label] !== false) {
      problems.push(`${label} reports hidden but its window is still visible`);
    }
    for (const [other] of others) {
      if (visible[other] === false) problems.push(`${other} is hidden as a window while ${label} is covered`);
    }
    console.log(
      `  ${label} (${screen}): hidden${others.length ? `, ${others.map(([l]) => l).join(", ")} left alone` : ""}` +
        ` — window visible=${visible[label] === false ? "no" : "yes"}, machine quiet=${state.machineQuiet}`,
    );
  }
  if (keepOpen) {
    console.log(`  --keep-open: leaving the probe on ${label} up`);
    break;
  }
  closeProbe(probe.status);
  // ...and everything comes back
  const back = await waitFor((s) => Object.values(s.byLabel).every((h) => !h), "both back");
  if (!back) problems.push(`the desktop did not come back after closing the probe on ${label}`);
}

if (problems.length) {
  console.error(`presence: FAILED — ${problems.length} problem(s)`);
  for (const p of problems) console.error(`  - ${p}`);
  await session.close();
  process.exit(1);
}
console.log("presence: each screen hid for its own fullscreen app and came back, the others untouched");
await session.close();
