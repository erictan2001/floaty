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
 * At the end it covers *every* screen at once for a moment: that is what "the machine is
 * quiet" means, and the wake afterwards is what has to restart the shared audio capture —
 * the Visualizer bug, where the page never heard the machine at all because its presence
 * listener was only ever registered from inside the display-changed handler.
 *
 * The app runs against a fixture world (see `runtime.mjs`), so the user's store and log are
 * left alone; the screens and the fullscreen windows are the real ones, which is the whole
 * point of the probe. It does put a window on one screen for a couple of seconds, and on all
 * of them at the end. Usage:
 *   node verify/presence.mjs [--keep-open]
 */
import { spawn, spawnSync } from "node:child_process";
import { readFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";
import os from "node:os";
import { Fixture, flag, launch, probe } from "./runtime.mjs";

const keepOpen = process.argv.includes("--keep-open");
const port = Number(flag("port", 9222));
const { skip, finish, problems } = probe("presence");
// Beside this file, not in a temp folder: a check that breaks when Temp is cleaned is not a
// check. The script is ASCII-only on purpose — PowerShell parses a BOM-less non-ASCII file
// in the console codepage and dies on the first dash it does not recognise.
const PROBE = fileURLToPath(new URL("./fullscreen-probe.ps1", import.meta.url));

const fixture = Fixture.create({ dir: join(os.tmpdir(), `floaty-presence-${process.pid}`) });

const app = await launch({ fixture, cwd: resolve(import.meta.dirname, ".."), port }).catch(
  (error) => skip(error.message),
);

// After `launch`, not before: exit handlers run in the order they were registered, and the
// one that stops the app has to run first — remove the folder while the app is still writing
// and it is back a moment later, with a fresh log in it.
process.on("exit", () => {
  if (!keepOpen) fixture.remove();
});

/** Whether the shared audio capture is up — the thing a per-screen answer must not break. */
const audioRunning = () => app.invoke("floaty_audio_status").then((up) => up === true);

const presence = () => app.invoke("floaty_presence");
const windows = async () => {
  const report = (await app.invoke("floaty_diagnostics")).windows ?? [];
  return Object.fromEntries(report.map((w) => [w.label, w.visible]));
};

/** Every desktop layer, and whether it is hidden — the whole point of the check. */
const hidden = async () => {
  const report = await presence();
  // A desktop layer is a window that belongs to a screen: the overlay windows, one per
  // screen. The app lists its panel windows (a note, the clock) in the same payload with
  // `screen: null` — they are windows *on* a screen, not layers *of* one, and the per-screen
  // rule is not about them. Including them made this probe try to cover "the screen of
  // widget-note-1" and report four failures on any install with a panel open.
  const layers = (report.screens ?? []).filter((s) => s.screen);
  return {
    byLabel: Object.fromEntries(layers.map((s) => [s.label, s.hidden])),
    byScreen: Object.fromEntries(layers.map((s) => [s.screen, s.hidden])),
    machineHidden: report.hidden,
    machineQuiet: report.quiet,
    foreground: report.foreground,
  };
};

/**
 * Wait for the state this is about to assert, and hand it back — or `null` if it never came.
 * Presence has to *decide* on the absence (a fullscreen window that hid nothing is a finding,
 * not a reason to stop), so this is `until` with the failure kept instead of thrown.
 */
const waitFor = (want, what, timeout) =>
  app
    .until(
      what,
      async () => {
        const state = await hidden();
        return want(state) ? state : null;
      },
      { timeout },
    )
    .catch(() => null);

const openFullscreen = (index, all = false) => {
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
    [
      "/c",
      "start",
      "",
      "powershell",
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      PROBE,
      ...(all ? ["-All"] : [String(index)]),
    ],
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
  spawnSync(
    "powershell",
    ["-NoProfile", "-Command", `Stop-Process -Id ${pid} -Force -ErrorAction SilentlyContinue`],
    {
      stdio: "ignore",
    },
  );
  return true;
};

/** Where the probe put its window, as the app would describe it: "2880x1920 at 0,0". */
const probeRect = (status) => {
  try {
    const text = readFileSync(status, "utf8");
    const size = /size=([0-9]+x[0-9]+)/.exec(text)?.[1];
    const at = /at=(-?[0-9]+,-?[0-9]+)/.exec(text)?.[1];
    return size && at ? `${size} at ${at}` : null;
  } catch {
    return null;
  }
};

/**
 * Someone is using the machine, so the probe's window is not the one in front — the rule
 * reads the *foreground* window, and a person at the keyboard keeps it. Say so and stop,
 * rather than report the app as broken.
 */
const busy = async (status) => {
  const mine = probeRect(status);
  if (!mine) return false;
  const now = await hidden();
  return Boolean(now.foreground) && now.foreground !== mine;
};

/** Close every probe at once: the machine phase runs one, but a straggler must not survive. */
const closeAll = () => {
  spawnSync(
    "powershell",
    [
      "-NoProfile",
      "-Command",
      'Get-CimInstance Win32_Process -Filter "Name=\'powershell.exe\'" | Where-Object { $_.CommandLine -like "*fullscreen-probe*" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }',
    ],
    { stdio: "ignore" },
  );
};

const before = await hidden();
const layers = Object.keys(before.byLabel);
console.log(`presence: ${layers.length} desktop layer(s): ${layers.join(", ")}`);
if (layers.length < 2) {
  skip("needs two desktop layers — this desktop has one screen");
}
// A fullscreen app already in front makes every assertion below meaningless — the desktop is
// hidden before the probe opens anything, and the "came back" checks measure that app, not
// the probe. Refuse, and say what to close.
const alreadyHidden = Object.entries(before.byLabel)
  .filter(([, isHidden]) => isHidden)
  .map(([l]) => l);
if (alreadyHidden.length) {
  skip(
    `${alreadyHidden.join(", ")} is already hidden by a fullscreen app in front (${before.foreground}) — ` +
      "close it and run this again",
  );
}

// One screen at a time, in the order the app labels them.
for (const [index, label] of layers.entries()) {
  const screen = (await presence()).screens.find((s) => s.label === label)?.screen;
  const run = openFullscreen(index);
  const state = await waitFor((s) => s.byLabel[label] === true, `fullscreen on ${label}`, 6);
  try {
    console.log(`  probe: ${readFileSync(run.status, "utf8").trim()}`);
  } catch {
    console.log("  probe: no status file — the window did not open");
  }
  if (!state) {
    if (await busy(run.status)) {
      closeProbe(run.status);
      skip(
        `${label} was not hidden because another window is in front — the probe put its window at ` +
          `${probeRect(run.status)} but the app sees "${(await hidden()).foreground}". Someone is using the ` +
          "machine; run this when the desktop is free.",
      );
    }
    problems.push(
      `a fullscreen window on ${label} (${screen}) did not hide it — ${JSON.stringify(await hidden())}`,
    );
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
    // The capture is shared and the state is per screen: a covered screen must not stop it.
    // Acting on the screen's own answer killed the visualizer on the screen that was still
    // showing, and nothing started it again because the machine had never gone quiet.
    // ...and only while the machine itself is awake: an idle machine is *supposed* to have
    // the capture stopped, so asserting on it then would fail for the right reason.
    if (!state.machineQuiet && !(await audioRunning())) {
      problems.push(
        `a fullscreen window on ${label} stopped the shared audio capture while ${others
          .map(([l]) => l)
          .join(", ")} still showed a desktop`,
      );
    }
    // The state is stored before the window is hidden, so a reader can see "hidden" a poll
    // before the hide lands. Wait for the fact this is asserting rather than sampling once.
    const visible =
      (await app
        .until(
          `${label}'s window to go invisible`,
          async () => {
            const v = await windows();
            return v[label] === false ? v : null;
          },
          { timeout: 2 },
        )
        .catch(() => null)) ?? (await windows());
    if (visible[label] !== false) {
      problems.push(`${label} reports hidden but its window is still visible`);
    }
    for (const [other] of others) {
      if (visible[other] === false)
        problems.push(`${other} is hidden as a window while ${label} is covered`);
    }
    console.log(
      `  ${label} (${screen}): hidden${others.length ? `, ${others.map(([l]) => l).join(", ")} left alone` : ""}` +
        ` — window visible=${visible[label] === false ? "no" : "yes"}, machine quiet=${state.machineQuiet},` +
        ` audio capture up=${await audioRunning()}`,
    );
  }
  if (keepOpen) {
    console.log(`  --keep-open: leaving the probe on ${label} up`);
    break;
  }
  closeProbe(run.status);
  // ...and everything comes back. Long enough to cover the probe closing itself, in case
  // closing it by pid did not take (its console is its own process).
  const back = await waitFor((s) => Object.values(s.byLabel).every((h) => !h), "both back", 25);
  if (!back) problems.push(`the desktop did not come back after closing the probe on ${label}`);
}

// ---- and the machine: every screen covered at once is what "quiet" means --------------
if (!keepOpen) {
  const wasUp = await audioRunning();
  // One window over every screen, not one per screen: two probes fight for the foreground,
  // and a window that covers each screen is what the rule needs to see.
  const covering = openFullscreen(0, true);
  const quiet = await waitFor((s) => s.machineQuiet, "the machine quiet", 15);
  if (!quiet) {
    if (await busy(covering.status)) {
      closeAll();
      skip(
        `covering every screen did nothing because another window is in front — the probe put its window at ` +
          `${probeRect(covering.status)} but the app sees "${(await hidden()).foreground}". Someone is using the ` +
          "machine; run this when the desktop is free.",
      );
    }
    problems.push("covering every screen did not make the machine quiet");
  } else {
    // The page has to *hear* it: the capture is shared, and the page is the only thing that
    // starts it again. Both halves are checked — the page's own line in the app log, and the
    // capture itself.
    const log = (await app.logLines(1000)).join("\n");
    const heard = (log.match(/\[presence\] page quiet=true/g) ?? []).length;
    if (heard < 1) {
      problems.push(
        "the pages never heard the machine go quiet — nothing would restart the audio capture on wake",
      );
    }
    if (await audioRunning()) {
      problems.push("the shared audio capture kept running while every screen was covered");
    }
    console.log(
      `  every screen covered: machine quiet=${quiet.machineQuiet}, pages that heard it=${heard}`,
    );
  }
  closeAll();
  const back = await waitFor(
    (s) => !s.machineQuiet && Object.values(s.byLabel).every((h) => !h),
    "awake",
    25,
  );
  if (!back) {
    problems.push("the machine did not come back after closing the probes");
  } else {
    // This is the fix: the capture is restarted by the page, and it has to actually come up.
    const up = await app
      .until("the audio capture to come back", async () => (await audioRunning()) || null, {
        timeout: 6,
      })
      .catch(() => null);
    if (!up) {
      problems.push("the audio capture did not come back when the machine woke up");
    } else {
      console.log(`  awake again: audio capture up=${up} (was ${wasUp})`);
    }
  }
}

if (problems.length) {
  console.error(`presence: FAILED — ${problems.length} problem(s)`);
  for (const p of problems) console.error(`  - ${p}`);
} else {
  console.log(
    "presence: each screen hid for its own fullscreen app and came back, the others untouched",
  );
}
await finish(
  () => app.stop(),
  () => (keepOpen ? undefined : fixture.remove()),
);
