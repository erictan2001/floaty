/**
 * The verification runtime: the world a probe runs in.
 *
 * `cdp.mjs` is the transport — the target list, one session, a key, a screenshot.
 * This is the layer every probe in this folder used to write for itself: which
 * overlay pages are open and whether they are *alive*, waiting for a state instead
 * of a stopwatch, the JSON-safe way to call a backend command, where a floatie is
 * being drawn, a mouse that goes through the real input pipeline, the exit ladder,
 * and a fixture world so a probe can test one without touching the user's desktop.
 *
 * The rules it exists to keep (README.md says them at length):
 *
 * - **Wait for the effect, not for a stopwatch.** `until()` is the only wait here.
 * - **A port that answers is not a live app.** `attach()` checks the Tauri bridge,
 *   because a killed app leaves a WebView2 behind that serves the port.
 * - **Exit 2 when the probe could not run**, never 1. `probe()` owns the ladder.
 * - **A probe owns its footprint, and the best footprint is none.** `Fixture`
 *   points the whole app at a scratch folder with `FLOATY_DATA_DIR`.
 *
 * Layering: it drives the app the way a person or the app's own windows do —
 * `invoke`, real input events, the app's own log — and reads the app's answers. It
 * does reach into the page for two facts the DOM is the only place that holds, both
 * of them reads and never a call: `drawnBy` asks which window has a slot, and
 * `grabPoint` asks where that slot is. There is no seam inside the app: every
 * command goes through the real IPC.
 */
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { APP_PORT, Session, freePort, listTargets, realErrors } from "./cdp.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** `--name value` out of `process.argv`, or `fallback`. */
export function flag(name, fallback, argv = process.argv.slice(2)) {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : fallback;
}

/**
 * The exit ladder, in one place: `0` the app is healthy, `1` the app failed a
 * check, `2` the probe could not run at all.
 *
 * The distinction is the whole reason this exists. A dead debug port, a corpse
 * WebView2 or a missing file is not an app that is up and wrong, and a probe that
 * reports one as the other trains everyone to ignore it.
 */
export function probe(name) {
  const problems = [];
  const check = (ok, what, detail = "") => {
    console.log(`  ${ok ? "ok  " : "FAIL"} ${what}${detail ? ` — ${detail}` : ""}`);
    if (!ok) problems.push(what);
    return ok;
  };
  return {
    problems,
    check,
    /** The probe could not run: say why and leave with 2. */
    skip(why) {
      console.error(`${name}: ${why}`);
      process.exit(2);
    },
    /** Done: 1 if anything failed, else 0. Always closes what it was given. */
    async finish(...closers) {
      for (const close of closers) {
        try {
          await close?.();
        } catch {
          /* closing is best-effort */
        }
      }
      process.exit(problems.length ? 1 : 0);
    },
  };
}

/** The overlay pages on the debug port, in the order the browser lists them. */
export async function overlays(port = APP_PORT) {
  const targets = await listTargets(port).catch(() => []);
  return targets.filter((t) => t.type === "page" && t.url.endsWith("#/overlay"));
}

/**
 * Whether two paths name the same file.
 *
 * The app's records can carry either separator — a root written with forward slashes
 * comes back as `C:/...\name` — so comparing strings only works while the fixture and
 * the probe happen to agree. `path.resolve` normalises both, which is what "the same
 * path" has to mean.
 */
export function samePath(a, b) {
  return path.resolve(a).toLowerCase() === path.resolve(b).toLowerCase();
}

/**
 * Attach to a *running* app: wait until at least `screens` overlay pages are open
 * and live, then hand back an `App`.
 *
 * The liveness check is the point. Killing the app leaves a WebView2 process
 * serving the debug port — the targets are listed, the URLs look right, and every
 * page is a corpse whose `__TAURI_INTERNALS__` is gone. That reads as an app bug
 * in every assertion that follows, so it is caught here and reported as what it is:
 * a probe that could not run.
 */
export async function attach({ port = APP_PORT, screens = 1, timeout = 60 } = {}) {
  const deadline = Date.now() + timeout * 1000;
  /** A page that answered but has no bridge: the corpse a killed app leaves behind. */
  let stale = false;
  /** A page whose document has not loaded yet — booting, not dead. */
  let booting = false;
  for (;;) {
    const pages = await overlays(port);
    if (pages.length >= screens) {
      const sessions = [];
      let live = 0;
      for (let nth = 0; nth < pages.length; nth += 1) {
        try {
          const s = await Session.open(port, "#/overlay", nth);
          // Two different failures, and telling them apart is the point: a corpse *throws*
          // ("Cannot read properties of undefined (reading 'invoke')" — the bridge died with
          // the app), while a page that is still loading answers `"undefined"`. Calling the
          // second one a corpse sends the reader off to kill a process that is fine.
          let bridge = null;
          try {
            bridge = await s.evaluate("return typeof window.__TAURI_INTERNALS__");
          } catch {
            bridge = "error";
          }
          if (bridge === "object") {
            live += 1;
            sessions.push(s);
          } else {
            if (bridge === "error") stale = true;
            else booting = true;
            await s.close();
          }
        } catch {
          stale = true;
        }
      }
      if (live >= screens) return new App(port, sessions);
      for (const s of sessions) await s.close();
    }
    if (Date.now() > deadline) {
      throw new Error(
        stale
          ? `port ${port} lists overlay pages but they are dead — a WebView2 left over from a ` +
              "previous run is holding the port. Kill it (or restart the app) and retry."
          : booting
            ? `the overlay page(s) on port ${port} have not finished loading after ${timeout}s — no ` +
                "Tauri bridge yet. A cold dev server can take a minute to serve the first module; give it " +
                "longer, or reload the page. A page hidden behind a fullscreen app is suspended by " +
                "WebView2 and never gets one at all — the app's log says 'desktop off screen' when that " +
                "is what happened."
            : `no live overlay page on port ${port} after ${timeout}s`,
      );
    }
    await sleep(1000);
  }
}

export class App {
  constructor(port, sessions) {
    this.port = port;
    /** one session per overlay page, in screen order as the browser lists them */
    this.sessions = sessions;
    /** where the mouse last was, so `moveTo` can take real steps */
    this.at = null;
  }

  /** The nth overlay page. One screen each, so this is a screen, not a window. */
  page(nth = 0) {
    const s = this.sessions[nth];
    if (!s) throw new Error(`verify: no overlay page ${nth} (there are ${this.sessions.length})`);
    return s;
  }

  /**
   * A backend command through the real IPC — `window.__TAURI_INTERNALS__.invoke`,
   * which is what the app's own windows use, so there is no test seam inside the
   * app and no second implementation of it here.
   *
   * The JSON round-trip is deliberate: CDP serialises the value, and the shapes
   * that come back from the store (records, screens, diagnostics) are plain JSON,
   * so this keeps one representation instead of whatever `returnByValue` decides.
   */
  async invoke(command, args = {}, nth = 0) {
    const text = await this.page(nth).evaluate(
      `return JSON.stringify(await window.__TAURI_INTERNALS__.invoke(` +
        `${JSON.stringify(command)}, ${JSON.stringify(args)}));`,
    );
    return text === undefined ? undefined : JSON.parse(text);
  }

  /** `invoke`, but a refusal comes back as `{ error }` rather than a throw. */
  async tryInvoke(command, args = {}, nth = 0) {
    return this.invoke(command, args, nth).catch((error) => ({ error: String(error) }));
  }

  /**
   * Wait until `check` returns something truthy, and hand it back.
   *
   * This is the rule the README states and the probes kept breaking: a fixed delay
   * is a guess at how long something takes, and it is wrong on a slow machine and
   * wasted on a fast one. Poll the effect instead, with a deadline that only
   * exists to give up.
   */
  async until(what, check, { timeout = 15, every = 150 } = {}) {
    const deadline = Date.now() + timeout * 1000;
    let last = null;
    for (;;) {
      try {
        last = await check();
        if (last) return last;
      } catch (error) {
        last = error;
      }
      if (Date.now() > deadline) {
        const error = new Error(
          `waited ${timeout}s for ${what} (last: ${last?.message ?? last ?? "nothing"})`,
        );
        // What the predicate last saw, for a probe that has to report *what* it timed
        // out on: `await until(...).catch((e) => e.last)`. Throwing it away made every
        // caller re-measure the same state it had just read.
        error.last = last;
        throw error;
      }
      await sleep(every);
    }
  }

  /** Every floatie the app knows about. */
  list() {
    return this.invoke("floaty_list");
  }

  /**
   * Where one floatie is, as `[x, y]`, or null when there is no such record.
   *
   * The record owns a floatie's position (CONTEXT.md: the record is the position, the
   * slot is a view), so this asks the record list rather than the pixels — the read
   * every drag probe used to hand-roll.
   */
  async placeOf(id) {
    const rec = (await this.list()).find((r) => r.id === id);
    return rec ? [rec.x, rec.y] : null;
  }

  /**
   * Where a record's item is on disk.
   *
   * The path key is the *kind's* to declare — `target` for a file floatie, `path`
   * for a folder, whatever an installed plugin says for its own kind — and the
   * manifest is where it is declared. Probes used to guess (`data.target ??
   * data.path`), which is a second rule that can only drift from the first, and it
   * fails as "the record has no path" rather than as "the probe guessed wrong".
   * Asked once, then remembered.
   */
  async pathOf(rec) {
    this.keys ??= new Map(
      (await this.invoke("floaty_plugins")).map((p) => [p.id, p.desktop_item?.path_key]),
    );
    const value = rec.data?.[this.keys.get(rec.kind) ?? ""];
    return typeof value === "string" ? value : "";
  }

  /** The screens, named, with their logical rectangles. */
  screens() {
    return this.invoke("floaty_screens");
  }

  /** The screens as flat logical rectangles — `{x, y, w, h}` each. */
  monitors() {
    return this.invoke("floaty_monitors");
  }

  /** The app's own log, oldest first, newest last. */
  async logLines(n = 200) {
    const d = await this.invoke("floaty_diagnostics");
    return (d?.log?.lines ?? []).slice(-n);
  }

  /**
   * Console errors and exceptions seen on every page, and *drained*: the next call
   * starts clean, so a probe can say "these happened during the thing I just did"
   * instead of blaming an error from its own setup on the assertion.
   *
   * Pass a page number to take one page's only. Errors belong to the screen that
   * raised them, and a probe that checks window by window wants to keep saying so —
   * sweeping every session mid-loop files all of them under the first window.
   */
  takeErrors(nth = null) {
    const all = [];
    for (let i = 0; i < this.sessions.length; i += 1) {
      if (nth !== null && i !== nth) continue;
      all.push(...realErrors(this.sessions[i].errors));
      this.sessions[i].errors.length = 0;
    }
    return all;
  }

  /**
   * Which overlay page is drawing the floatie with this id — by the DOM, which is
   * the only place that answers it. A window is one screen, so an id drawn by two
   * pages is drawn twice, and an id drawn by none is invisible even when its record
   * is fine.
   */
  async drawnBy(id) {
    const out = [];
    for (let nth = 0; nth < this.sessions.length; nth += 1) {
      const here = await this.page(nth)
        .evaluate(`return document.getElementById("slot-" + ${JSON.stringify(id)}) !== null;`)
        .catch(() => false);
      if (here) out.push(nth);
    }
    return out;
  }

  /** Which screen a record's stored place is on, by the app's own rectangles. */
  async screenOf(record) {
    const screens = await this.screens();
    return (
      screens.find(
        (s) =>
          record.x >= s.logical.x &&
          record.x < s.logical.x + s.logical.w &&
          record.y >= s.logical.y &&
          record.y < s.logical.y + s.logical.h,
      )?.name ?? null
    );
  }

  /**
   * A real mouse event, through the input pipeline a person's mouse uses.
   *
   * `nth` is which overlay page gets it, and it is not a detail: one window draws one
   * screen's floaties, so on a two-screen desktop the floatie being dragged is usually
   * not on page 0, and an event sent there lands on the wrong monitor.
   */
  async mouse(type, x, y, nth = 0) {
    this.at = { x: Math.round(x), y: Math.round(y) };
    await this.page(nth).send("Input.dispatchMouseEvent", {
      type,
      x: this.at.x,
      y: this.at.y,
      button: "left",
      buttons: type === "mouseReleased" ? 0 : 1,
      clickCount: type === "mouseReleased" ? 1 : 0,
    });
  }

  /** Move the pointer there in steps, so the page sees a drag and not one jump. */
  async moveTo(x, y, { steps = 8, from = this.at, nth = 0 } = {}) {
    const start = from ?? { x, y };
    for (let step = 1; step <= steps; step += 1) {
      await this.mouse(
        "mouseMoved",
        start.x + ((x - start.x) * step) / steps,
        start.y + ((y - start.y) * step) / steps,
        nth,
      );
    }
  }

  /**
   * Where to press to pick a floatie up, and what that grabbed: a panel with a
   * title bar is dragged by the bar. The middle of a note is its text, and dragging
   * text must not move the note — the app is right to refuse, and a probe that
   * grabs the middle proves nothing.
   */
  grabPoint(id, nth = 0) {
    return this.page(nth).evaluate(`(() => {
      const el = document.getElementById("slot-" + ${JSON.stringify(id)});
      if (!el) return null;
      const target = el.querySelector(".bar") ?? el;
      const r = target.getBoundingClientRect();
      return {
        x: Math.round(r.x + r.width / 2),
        y: Math.round(r.y + r.height / 2),
        grabbed: target === el ? "the panel" : "its title bar",
      };
    })()`);
  }

  /** A PNG of one page. */
  shot(file, nth = 0) {
    return this.page(nth).screenshot(file);
  }

  async close() {
    for (const s of this.sessions) await s.close();
  }
}

/**
 * A scratch world for a probe: its own data folder, its own root, its own log.
 *
 * The app resolves all of it through one helper, which honours `FLOATY_DATA_DIR`,
 * so pointing that at `<dir>/data` moves the store, the settings, the plugins
 * folder, the icon cache and the log in one move. The root is not special-cased
 * either: it is a string in the settings file, so the fixture writes one that names
 * a folder it made itself.
 *
 * What that buys, beyond not touching the user's desktop: the content is known, so
 * "the probe left the world as it found it" becomes an exact assertion instead of a
 * hopeful one — and a probe can be run twice in a row and mean the same thing.
 */
export class Fixture {
  /**
   * Make the world. `files` is a list of names, written into the root; `dirs` are
   * folders made there. Nothing is written outside `dir`.
   */
  static create({ dir, files = ["alpha.txt", "beta.txt"], dirs = ["sub"], shortcut = "Ctrl+Alt+F9" } = {}) {
    if (!dir) throw new Error("verify: a fixture needs a dir");
    fs.rmSync(dir, { recursive: true, force: true });
    const data = path.join(dir, "data");
    const root = path.join(dir, "root");
    fs.mkdirSync(data, { recursive: true });
    fs.mkdirSync(root, { recursive: true });
    for (const name of files) fs.writeFileSync(path.join(root, name), `fixture ${name}\n`);
    for (const name of dirs) fs.mkdirSync(path.join(root, name, "deep"), { recursive: true });
    // A partial settings file is enough — every other field has a serde default —
    // but `root_confirmed` is not optional: without it the first-run guard blocks
    // and the probe would be measuring the guard.
    fs.writeFileSync(
      path.join(data, "floaty-settings.json"),
      JSON.stringify(
        {
          // The native form, backslashes and all: the app appends to this string, so a
          // root written with forward slashes comes back in records as `C:/...\name` —
          // a path no probe can compare against one it built with `path.join`.
          files_root: root,
          root_confirmed: true,
          // Not the user's chord: a second instance registering the same global
          // hotkey loses the race, and `floaty_palette_state.live` would report it.
          palette_shortcut: shortcut,
          start_on_boot: false,
          stay_on_desktop: true,
        },
        null,
        2,
      ),
    );
    fs.writeFileSync(path.join(data, "floaty-store.json"), "[]");
    return new Fixture({ dir, data, root, files, dirs });
  }

  constructor({ dir, data, root, files, dirs }) {
    this.dir = dir;
    this.data = data;
    this.root = root;
    this.files = files;
    this.dirs = dirs;
  }

  /** The environment the app has to be started with for this world to be used. */
  get env() {
    return { FLOATY_DATA_DIR: this.data };
  }

  /** A path inside the root, for a probe's own throwaway file. */
  inRoot(name) {
    return path.join(this.root, name);
  }

  remove() {
    fs.rmSync(this.dir, { recursive: true, force: true });
  }
}

/**
 * Start the app against a fixture world and attach to it.
 *
 * `dev` runs the app the way this repo does (`npm run tauri dev`), which is what a
 * probe wants: the same overlay, the same IPC, the same watcher as the thing being
 * verified. The user's own app must not be running at the same time — two overlays
 * stack, and the fixture's shortcut exists so at least the hotkey is not a fight.
 */
export async function launch({ fixture, cwd, port = APP_PORT, timeout = 240, log = console.log } = {}) {
  // A probe must drive the app it started. An app already on the port — the user's,
  // or a corpse from a run that was interrupted — would answer every call and make
  // this probe report on a world it never made.
  const already = await overlays(port);
  if (already.length) {
    throw new Error(
      `verify: ${already.length} overlay page(s) are already on port ${port} — an app is running. ` +
        "Stop it (the user's own, or a leftover from an interrupted run) and retry.",
    );
  }

  // A dev server of its own, on a free port. The user's `tauri dev` usually holds 1420,
  // and a `beforeDevCommand` that cannot bind takes the whole launch down with it — so the
  // fixture app gets its own Vite instead of sharing (or fighting over) theirs. `--config`
  // merges over tauri.conf.json and takes a *path*, which is one less thing for the shell
  // to mangle than a JSON string.
  const devPort = await freePort(1421);
  const configPath = path.join(os.tmpdir(), `floaty-verify-launch-${process.pid}.json`);
  fs.writeFileSync(
    configPath,
    JSON.stringify({
      build: {
        devUrl: `http://localhost:${devPort}`,
        beforeDevCommand: `npm run dev -- --port ${devPort} --strictPort`,
      },
    }),
  );

  const child = spawn(`npm run tauri dev -- --config "${configPath}"`, {
    cwd,
    shell: true,
    env: {
      ...process.env,
      ...(fixture?.env ?? {}),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
      // Its own cargo target dir: the user's `tauri dev` holds `target/debug/floaty.exe`
      // open, and Windows will not let a build replace a running binary — the failure is
      // "failed to remove floaty.exe: Access is denied", from cargo, not from the probe.
      // One shared fixture target inside the gitignored `target/`, so the first fixture
      // run pays for a build and the rest are incremental.
      CARGO_TARGET_DIR: process.env.FLOATY_VERIFY_TARGET ?? path.join(cwd, "src-tauri", "target", "verify"),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (b) => (output += b.toString()));
  child.stderr.on("data", (b) => (output += b.toString()));

  // The app must not outlive the probe, however the probe ends. An uncaught throw would
  // otherwise leave an overlay on the user's screen and a debug port for the next run to
  // attach to — and the port would answer, which is the trap `attach()` exists to catch.
  // The exit path gets its own synchronous version because there is no await left at that
  // point, and it walks the tree first for the same reason `stop()` does.
  process.on("exit", () => {
    try {
      fs.rmSync(configPath, { force: true });
    } catch {
      /* already gone */
    }
    if (child.killed) return;
    if (process.platform === "win32" && child.pid) {
      try {
        spawnSync("taskkill", ["/F", "/T", "/PID", String(child.pid)], { stdio: "ignore" });
      } catch {
        /* already gone */
      }
    }
    try {
      child.kill();
    } catch {
      /* already gone */
    }
  });

  const app = await (async () => {
    const deadline = Date.now() + timeout * 1000;
    for (;;) {
      try {
        return await attach({ port, screens: 1, timeout: 5 });
      } catch (error) {
        if (Date.now() > deadline) {
          await stop(child, port);
          throw new Error(`verify: the app never came up — ${error.message}\n${output.slice(-2000)}`);
        }
        await sleep(2000);
      }
    }
  })();
  app.child = child;
  app.stop = async () => {
    // The sockets first: an open WebSocket keeps node's event loop alive, so a probe
    // that stops the app and nothing else sits there after its last line — looking
    // exactly like a hung app, which is the worst way for a probe to fail.
    await app.close();
    await stop(child, port);
    try {
      fs.rmSync(configPath, { force: true });
    } catch {
      /* already gone */
    }
  };
  log(`launched the app against ${fixture ? fixture.dir : "the real install"}`);
  return app;
}

/** Stop a launched app: the tree, and the WebView2 that can outlive it. */
async function stop(child, port) {
  if (!child || child.killed) return;
  const pid = child.pid;
  // The tree first, while the parent is still alive to be walked: `taskkill /T`
  // enumerates children through it, so killing npm first leaves vite and the app
  // running with no parent to find them by — which is how a probe ends up attaching
  // to the previous run's app.
  if (process.platform === "win32" && pid) {
    await new Promise((resolve) => {
      spawn("taskkill", ["/F", "/T", "/PID", String(pid)], { stdio: "ignore" }).on("close", resolve);
    });
  }
  try {
    child.kill();
  } catch {
    /* already gone */
  }
  // The wait here is not a guess at how long the process takes to go — it is for the
  // port. A WebView2 can keep answering after the app that owned it is dead, and a probe
  // that attaches to that answers every call while reporting on a world it never made, so
  // poll for the port going quiet and stop as soon as it does. The deadline only exists
  // to give up: a WebView2 that never lets go costs the next probe a corpse it has to
  // report, which is its own problem and not worth a stopwatch here.
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    if (!(await overlays(port)).length) break;
    await sleep(100);
  }
}
