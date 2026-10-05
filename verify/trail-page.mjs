/**
 * The trail's page and drawing surface: does a panel that borrowed the desktop give it
 * back?
 *
 * The bug this exists for is invisible to a unit test and obvious to a person: click
 * **load**, press escape, and the panel does not come back to its own size and place —
 * it stays stretched over the whole screen. Neither half is the plugin's mistake alone:
 *
 * - `api.setPos(id, x, y, {transient: true})` had a way to say *this stretch is
 *   scaffolding, do not record it*. `api.setSize(id, w, h)` did not. Since the session
 *   refactor (2da2f62), a `setSize` on your own widget writes `data.w/h` — so covering
 *   the desktop overwrote the panel's real size in the record.
 * - and the restore read its destination out of that record. Restoring from the record
 *   you are restoring *from* restores the stretch you just undid.
 *
 * Both halves are silent: the slot looks right while the page is open, the record is
 * the only place the damage shows, and a `save` right after makes it permanent. So the
 * probe asserts both — the slot's drawn size, and the size the saved record ended up
 * with — and does it for **both** gestures that stretch the slot: the save page and the
 * drawing surface. Fixing one and not the other is the easy mistake here.
 *
 * The world: the overlay page is driven in a headless Chrome with the IPC stubbed, so
 * nothing is written to the user's store, data folder or log — the store this stub
 * answers *is* the world, and it holds exactly one record: the panel under test. A
 * desktop full of icons would only make the numbers harder to read.
 *
 *     node verify/trail-page.mjs
 *
 * The dev server is its own, on a free port and with `--force`: Vite caches modules
 * outside `src/`, and `/examples/plugins/trail/index.js` is served from the project
 * root — an edit that keeps coming back as the previous version reads exactly like a
 * fix that did nothing.
 */

import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { freePort, launchChrome } from "./cdp.mjs";
import { App, probe } from "./runtime.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const cwd = path.join(here, "..");

/** The trail's own size — `examples/plugins/trail/plugin.json` says 268x182. */
const PANEL = { w: 268, h: 182 };
/** Where the record puts the panel, well inside the screen. */
const HOME = { x: 40, y: 40 };
/** The screen the stub reports. A stretch has to cover *something* deterministic, or
 *  the assertion is "not the window" and that passes for the wrong reason. */
const SCREEN = { w: 1280, h: 800 };
const ID = "trail-1";

const { check, skip, finish } = probe("trail-page");

/**
 * The stub, with the trail's module entry filled in before it is injected.
 *
 * It mirrors the shapes the real commands return — `default_size` as a pair,
 * `desktop_item` as the manifest's object — because a stub that omits a field makes the
 * page look fine here and break in the app. The trail answers as an *installed* plugin
 * with a real `entry`, so the page imports the module the way it does in the app.
 */
const stubFor = (entry) => `(() => {
  const callbacks = new Map();
  let nextCb = 1;
  const SCREEN = { x: 0, y: 0, w: ${SCREEN.w}, h: ${SCREEN.h} };
  const plugin = (id, extra) => Object.assign({
    id, name: id, description: id, enabled: true, default_size: [268, 182],
    resizable: true, desktop_item: null, add_label: "+ " + id, layout_priority: 7,
    source: "installed", entry: null, version: "1.0.0", author: "floaty",
    api_version: 2, trusted: true,
  }, extra || {});
  // The panel under test, and the only record there is.
  let panel = { id: ${JSON.stringify(ID)}, kind: "trail", x: ${HOME.x}, y: ${HOME.y},
    data: { w: ${PANEL.w}, h: ${PANEL.h}, ids: [],
      // one saved slot, so "forget this slot" is reachable: it is the cheapest action
      // that saves the record the panel is holding, and so the cheapest way a person
      // can make a stretch permanent without noticing
      saves: [{ name: "trail 1", at: 1, paths: [{ x: 100, y: 100 }, { x: 300, y: 200 }], thumb: "" }] } };
  window.__SAVES__ = [];
  window.__CALLS__ = [];
  window.__LOG__ = [];
  const answer = async (cmd, args) => {
    switch (cmd) {
      case "floaty_get_settings": return {
        gravity: 2600, bounce: 0.45, single_click: "drop", double_click: "launch",
        live2d_root: "", files_root: "", disabled: [], stay_on_desktop: true,
        start_on_boot: false, animated_ratio: 100, animation_mode: "wave",
        float_amplitude: 6, float_period: 10, float_spread: 10, confirm_remove: false,
        viz_gain: 1, viz_fps: 30, sysmon_interval: 1000, icon_pipeline: 4,
        palette_shortcut: "Ctrl+Alt+F9", plugin_trust: {},
      };
      case "floaty_monitors": return [{ x: 0, y: 0, w: SCREEN.w, h: SCREEN.h, scale: 1, primary: true }];
      case "floaty_overlay_area": return {
        index: 0, desktop: SCREEN,
        screen: { name: "display1", primary: true, scale: 1, physical: SCREEN, logical: SCREEN },
      };
      case "floaty_screens": return [{ name: "display1", logical: SCREEN, physical: SCREEN }];
      // The first-run guard blocks the desktop until it has an answer, and every field
      // of the report is one it reads: an omitted \`confirmed\` is a screen the probe would
      // be measuring the guard through instead of the panel.
      case "floaty_root_guard": return {
        confirmed: true, root: "C:/fixture/root", exists: true,
        items: 2, files: 1, folders: 1, apps: 0, names: ["alpha.txt", "sub"], more: 0,
      };
      case "floaty_get_record":
        return args && args.id === panel.id ? JSON.parse(JSON.stringify(panel)) : null;
      case "floaty_list": return [JSON.parse(JSON.stringify(panel))];
      case "floaty_plugins": return [
        plugin("note"), plugin("clock"), plugin("live2d"), plugin("visualizer"), plugin("sysmon"),
        plugin("app", { desktop_item: { path_key: "target", noun: "app floatie", group: false } }),
        plugin("file", { desktop_item: { path_key: "target", noun: "file floatie", group: false } }),
        plugin("folder", { desktop_item: { path_key: "path", noun: "folder floatie", group: true } }),
        plugin("trail", { entry: ${JSON.stringify(entry)}, default_size: [${PANEL.w}, ${PANEL.h}] }),
      ];
      case "floaty_save": {
        // Keep what the backend would keep, and record every write: the record's own
        // idea of its size is the half of this bug that shows up nowhere else.
        const r = args && args.record;
        if (r && r.id === panel.id) panel = JSON.parse(JSON.stringify(r));
        window.__SAVES__.push(r ? { id: r.id, x: r.x, y: r.y, w: r.data && r.data.w, h: r.data && r.data.h } : null);
        return null;
      }
      case "floaty_log": window.__LOG__.push((args && args.msg) || ""); return null;
      case "plugin:event|listen": return 1;
      default: return null;
    }
  };
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "desktop-overlay" }, currentWebview: { label: "desktop-overlay" } },
    transformCallback: (cb) => { const id = nextCb++; callbacks.set(id, cb); return id; },
    unregisterCallback: (id) => callbacks.delete(id),
    // The app's version percent-encodes an absolute path onto the asset protocol; here
    // the module is served over http so the page can import it exactly as it does there.
    convertFileSrc: (p) => p,
    invoke: async (cmd, args) => { window.__CALLS__.push(cmd); return answer(cmd, args); },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
})()`;

/** The panel's slot geometry and what is drawn on top of it, read from the live page. */
const MEASURE = `(() => {
  const el = document.getElementById("slot-${ID}");
  if (!el) return { mounted: false };
  const r = el.getBoundingClientRect();
  const panel = el.querySelector(".tr-panel");
  return {
    mounted: true,
    drawn: { w: Math.round(r.width), h: Math.round(r.height) },
    at: { x: Math.round(r.x), y: Math.round(r.y) },
    panelShown: !!panel && getComputedStyle(panel).display !== "none",
    pageOpen: !!document.querySelector(".tr-page"),
    surface: !!document.body.querySelector("canvas"),
  };
})()`;

const isPanel = (m) => m?.drawn?.w === PANEL.w && m?.drawn?.h === PANEL.h;
/**
 * How a measurement reads on a line — and it must read the same whether it passed or
 * failed, or a failing run says "nothing" where a passing one says "268x182 at 40,40"
 * and the two cannot be compared.
 */
const where = (m) =>
  m?.mounted ? `${m.drawn.w}x${m.drawn.h} at ${m.at.x},${m.at.y}` : "the panel was not there";

let vite;
let chrome;
try {
  const devPort = await freePort(1421);
  vite = spawn("npm", ["run", "dev", "--", "--port", String(devPort), "--strictPort", "--force"], {
    cwd,
    shell: true,
    stdio: "ignore",
  });
  const base = `http://localhost:${devPort}`;
  const up = await (async () => {
    const deadline = Date.now() + 90_000;
    for (;;) {
      try {
        if ((await fetch(`${base}/index.html`)).ok) return true;
      } catch {
        /* not up yet */
      }
      if (Date.now() > deadline) return false;
      await new Promise((r) => setTimeout(r, 400));
    }
  })();
  if (!up) skip(`the dev server never came up on ${devPort}`);

  const entry = `${base}/examples/plugins/trail/index.js`;
  chrome = await launchChrome({ inject: stubFor(entry), window: "1280,860" });
  const page = new App(chrome.port, [chrome.session]);
  const read = () => chrome.session.evaluate(MEASURE).catch(() => null);

  // A query as well as the hash, so every run is a real page load: navigating between
  // two urls that differ only after `#` is a same-document navigation, and the probe
  // would measure the previous run's DOM.
  await chrome.session.send("Page.navigate", { url: `${base}/index.html?run=${Date.now()}#/overlay` });

  // Wait for the thing being measured, never for a stopwatch: a fixed delay after
  // `Page.navigate` measures a half-loaded page.
  const mounted = await page
    .until("the trail panel to mount", async () => {
      const m = await read();
      return m?.mounted && m.panelShown ? m : null;
    }, { timeout: 45 })
    .catch((e) => e.last);
  check(!!mounted?.panelShown, "the trail panel mounted and drew itself", mounted ? "yes" : "no");
  if (!mounted?.panelShown) {
    const log = await chrome.session.evaluate("return (window.__LOG__ ?? []).slice(-6)").catch(() => null);
    if (log) console.log(`       the page logged: ${JSON.stringify(log)}`);
    await finish(chrome.stop, () => vite.kill());
  }

  /** Press one of the panel's buttons, by its label. */
  const click = (label) =>
    chrome.session.evaluate(
      `const b = [...document.querySelectorAll("#slot-${ID} .tr-btn")]
         .find((x) => x.textContent === ${JSON.stringify(label)});
       if (!b) return false;
       b.click();
       return true;`,
    );

  /**
   * Wait for `pred`, and if it never comes true, hand back what the page *actually*
   * looks like once the wait is over.
   *
   * `until` reports the last thing its predicate saw, and a predicate of "the panel is
   * its own size" returns null forever when it is not — so the timeout path said
   * "nothing" and the report could not tell a broken widget from a missing one. Reading
   * the page once more turns the failure into the number that matters: 1280x800 at 0,0
   * is the bug, and it reads the same in a passing run's log.
   */
  const settle = async (what, pred, timeout = 15) => {
    const hit = await page.until(what, pred, { timeout }).catch(() => null);
    if (hit) return hit;
    const last = await read();
    return last && { ...last, timedOut: true };
  };

  // ---- 1. at rest --------------------------------------------------------
  const rest = await page.until(
    "the panel's own size",
    async () => {
      const m = await read();
      return isPanel(m) ? m : null;
    },
    { timeout: 20 },
  );
  console.log(`  …    at rest: ${where(rest)}`);
  check(
    rest.at.x === HOME.x && rest.at.y === HOME.y,
    "the panel sits where its record says",
    `${rest.at.x},${rest.at.y}`,
  );

  // ---- 2. the reported case: load, then escape ---------------------------
  check(await click("load"), "the load button is there");
  const opened = await settle("the slot to stretch", async () => {
    const m = await read();
    return m?.pageOpen && m.drawn.w === SCREEN.w ? m : null;
  });
  check(!!opened?.pageOpen, "the page of slots opened", where(opened));
  check(
    !!opened && opened.drawn.w === SCREEN.w && opened.drawn.h === SCREEN.h,
    "and the slot is stretched over the whole screen",
    where(opened),
  );

  // A real key: a synthetic KeyboardEvent has no default action, and the save page's
  // escape handler is a window-level capture listener a stubbed event can fake.
  await chrome.session.key("escape");
  const closed = await settle("the panel to come back", async () => {
    const m = await read();
    return m && !m.pageOpen && m.panelShown && isPanel(m) ? m : null;
  });
  // One line carries all three: whether the page closed, how big the panel is, and where
  // it is. They are one fact — a panel that came back is a panel that came back *to* —
  // and splitting them across three lines made a failing run read as three failures.
  check(
    isPanel(closed) && !closed?.pageOpen && closed?.at.x === HOME.x && closed?.at.y === HOME.y,
    "escape closed the page and the panel is itself again, where it was",
    where(closed),
  );

  // The silent half: the record. It is what a later launch and a later restore both read,
  // and nothing above can see it — a slot that looks right while the page is open is
  // exactly how the bug hides. Closing the page must not need to *save* anything (with
  // the stretch transient, the record was never changed), so the assertion is on the
  // record as the backend still holds it, plus a scan of every write that did happen.
  const stored = async () =>
    (await chrome.session.evaluate(
      `return JSON.stringify(await window.__TAURI_INTERNALS__.invoke("floaty_get_record", { id: ${JSON.stringify(ID)} }));`,
    )) ?? "{}";
  const afterClose = JSON.parse(await stored());
  check(
    afterClose.data?.w === PANEL.w && afterClose.data?.h === PANEL.h,
    "the record still says the panel is its own size",
    `${afterClose.data?.w}x${afterClose.data?.h}`,
  );
  check(
    afterClose.x === HOME.x && afterClose.y === HOME.y,
    "and that it lives where the user put it",
    `${afterClose.x},${afterClose.y}`,
  );
  const stretchWrites = (await chrome.session.evaluate(
    "return JSON.stringify(window.__SAVES__.filter((s) => s && (s.w > 400 || s.h > 400)))",
  )) ?? "[]";
  check(
    JSON.parse(stretchWrites).length === 0,
    "and no write ever recorded the screen's size as the panel's",
    stretchWrites === "[]" ? "none" : stretchWrites,
  );

  // ---- 3. the same gesture, other shape: a drawing surface --------------
  check(await click("single"), "the single button is there");
  const surface = await settle("the surface to stretch", async () => {
    const m = await read();
    return m?.surface && m.drawn.w === SCREEN.w ? m : null;
  });
  check(!!surface?.surface, "the drawing surface opened", where(surface));
  check(
    !!surface && surface.drawn.w === SCREEN.w && surface.drawn.h === SCREEN.h,
    "and the slot is stretched over the whole screen",
    where(surface),
  );

  await chrome.session.key("escape");
  const back = await settle("the panel to come back from the surface", async () => {
    const m = await read();
    return m && !m.surface && m.panelShown && isPanel(m) ? m : null;
  });
  check(
    isPanel(back) && !back?.surface && back?.at.x === HOME.x && back?.at.y === HOME.y,
    "escape closed the surface and the panel is itself again, where it was",
    where(back),
  );
  const savedAfter = JSON.parse(await stored());
  check(
    savedAfter.data?.w === PANEL.w && savedAfter.data?.h === PANEL.h,
    "and the record still says so after the drawing surface too",
    `${savedAfter.data?.w}x${savedAfter.data?.h}`,
  );
  const allWrites = JSON.parse(
    (await chrome.session.evaluate(
      "return JSON.stringify(window.__SAVES__.filter((s) => s && (s.w > 400 || s.h > 400)))",
    )) ?? "[]",
  );
  check(allWrites.length === 0, "still no write recorded the screen's size", JSON.stringify(allWrites));

  // ---- 4. the save that makes it permanent -----------------------------
  // Neither gesture *saves* on its own, and each open re-reads the record — which is
  // why the bug above looks like a redraw glitch and then goes away by itself. The
  // in-memory record is the thing that is wrong: the closing `setSize` wrote the
  // screen's size into it, and that copy is what every plugin action saves.
  //
  // So the sequence is: stretch the slot, and then save the record *while it is still
  // stretched*. "Forget this slot" is exactly that — `api.record.save(rec)` with no
  // re-read, one click, inside the open page. That is what turns a panel that was
  // briefly the size of the screen into one that is on the next launch, forever.
  await click("load");
  await settle("the page to open for the save", async () => (await read())?.pageOpen);
  check(
    isPanel((await read())) === false,
    "the slot is stretched while the page is open",
    where(await read()),
  );
  const forgot = await chrome.session.evaluate(
    `const x = [...document.querySelectorAll(".tr-page .tr-x:not(.tr-edit)")][0];
     if (!x) return false;
     x.click();
     return true;`,
  );
  check(forgot, "a slot was forgotten, which saves the record the panel is holding");
  await chrome.session.key("escape");
  await settle("the panel to come back after it", async () => {
    const m = await read();
    return m && !m.pageOpen ? m : null;
  });
  const afterSave = JSON.parse(await stored());
  check(
    afterSave.data?.w === PANEL.w && afterSave.data?.h === PANEL.h,
    "the record is still the panel's own size — the stretch did not become permanent",
    `${afterSave.data?.w}x${afterSave.data?.h}`,
  );

  const errors = page.takeErrors();
  check(errors.length === 0, "no console errors along the way", errors.slice(0, 2).join(" | "));
} catch (err) {
  console.error(`  FAIL ${err?.stack ?? err}`);
  check(false, "the probe ran to the end", String(err?.message ?? err));
} finally {
  await finish(chrome?.stop, () => vite?.kill());
}