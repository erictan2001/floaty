/**
 * Render every settings pane for real, with the Tauri IPC stubbed, and report
 * what each one drew.
 *
 * Run with the Vite dev server up (any port; pass it as the first argument):
 *
 *     npm run dev                       # in one terminal, :1420
 *     npm run verify:panes              # in another
 *
 * Why this exists: the settings window is a plain webview page, so the whole UI
 * can be exercised without the app — and without a second floaty fighting over
 * `floaty-store.json`. What it catches that `tsc` cannot: a pane that throws while
 * drawing (an unhandled promise, a field the stub did not answer), a row that
 * overflows a 380px window, a button that disappears when its card re-renders.
 *
 * The stub answers the same commands the backend does, with plausible values, and
 * records every call — so an action's invoke *sequence* can be asserted too
 * (`floaty_set_settings` then `floaty_sync_files`, and in that order).
 */

import path from "node:path";
import { fileURLToPath } from "node:url";
import { launchChrome, realErrors } from "./cdp.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const base = (process.argv[2] ?? "http://localhost:1420").replace(/\/$/, "");
const shiny = process.argv.includes("--shot");
const only = process.argv.slice(3).filter((a) => !a.startsWith("--"));

/** The panes a run covers; `npm run verify:panes -- http://localhost:1420 desktop`. */
export const PANES = ["desktop", "apps", "files", "motion", "plugins", "general"].filter(
  (pane) => only.length === 0 || only.includes(pane),
);


/**
 * The stub. Keep it honest about *shape*: a field a real command returns but this
 * omits is a pane that looks fine here and breaks in the app, which is the one
 * way a probe like this is worse than nothing.
 */
export const STUB = `(() => {
  const callbacks = new Map();
  let nextCb = 1;
  const plugin = (id, name, extra) => Object.assign({
    id, name, description: name + " description", enabled: true, default_size: [200, 140],
    resizable: true, desktop_item: null, add_label: "+ " + id, layout_priority: 10,
    source: "builtin", entry: null, version: "", author: "",
  }, extra || {});
  const widget = (id, kind, data) => ({ id, kind, x: 40, y: 60, data: data || {} });
  const settings = {
    pet_speed: 1, gravity: 2600, bounce: 0.45, single_click: "drop", double_click: "launch",
    live2d_root: "", files_root: "C:\\\\Users\\\\erict\\\\OneDrive\\\\Desktop", disabled: [],
    stay_on_desktop: true, start_on_boot: false, animated_ratio: 100, animation_mode: "wave",
    float_amplitude: 6, float_period: 10, float_spread: 10, confirm_remove: true,
    viz_gain: 1, viz_fps: 30, sysmon_interval: 1000, icon_pipeline: 4,
  };
  const answer = async (cmd, args) => {
    switch (cmd) {
      case "floaty_get_settings": return settings;
      case "floaty_undo_state": return { depth: 2, label: "delete" };
      case "floaty_plugins": return [
        plugin("note", "Note"), plugin("clock", "Clock"), plugin("pet", "Pet"),
        plugin("app", "App", { desktop_item: { path_key: "target", noun: "app floatie", group: false } }),
        plugin("file", "File", { desktop_item: { path_key: "target", noun: "file floatie", group: false } }),
        plugin("folder", "Folder", { desktop_item: { path_key: "path", noun: "folder floatie", group: true } }),
        plugin("live2d", "Live2D", { resizable: false }),
        plugin("visualizer", "Visualizer"), plugin("sysmon", "System monitor"),
        plugin("countdown", "Countdown", { source: "installed", version: "1.0.0", author: "you", add_label: "+ countdown" }),
        plugin("trail", "Trail", { source: "installed", version: "1.0.0", author: "floaty", add_label: "+ trail", resizable: true }),
      ];
      case "floaty_list": return [
        widget("app-1", "app", { name: "Arc", target: "C:\\\\Arc.lnk", icon: "", pinned: true }),
        widget("file-2", "file", { name: "notes.txt", target: "C:\\\\notes.txt", icon: "" }),
        widget("folder-3", "folder", { name: "Games", path: "C:\\\\Games", items: [{ name: "A.lnk", target: "C:\\\\Games\\\\A.lnk", icon: "", is_dir: false }] }),
        widget("note-4", "note", { text: "hello" }),
        widget("clock-5", "clock", {}),
        widget("trail-6", "trail", { paths: [] }),
      ];
      case "floaty_plugins_dir": return "C:\\\\Users\\\\erict\\\\AppData\\\\Roaming\\\\com.floaty.app\\\\plugins";
      case "floaty_scan_apps": return [
        { name: "Arc", path: "C:\\\\Arc.lnk", source: "Start Menu" },
        { name: "Visual Studio Code", path: "C:\\\\Code.lnk", source: "Desktop" },
      ];
      case "floaty_scan_models": return [{ name: "hiyori", path: "C:\\\\models\\\\hiyori", folder: "hiyori" }];
      case "floaty_inspect_plugin": return {
        id: "countdown", name: "Countdown", version: "1.1.0", author: "you",
        description: "Counts down to a date you pick.", api_version: 1, add_label: "+ countdown",
        files: 2, size_bytes: 4211, installed_version: "1.0.0", relation: "upgrade",
      };
      case "floaty_uninstall_plugin": return { id: "countdown", files: 2, recycled_to: "C:\\\\plugins\\\\countdown" };
      case "floaty_palette_state": return { accelerator: "Ctrl+Alt+Space", live: true, visible: false };
      case "floaty_sync_files": return { added: 0, removed: 0, refreshed: 0 };
      case "floaty_icon": return "";
      case "plugin:event|listen": return 1;
      case "plugin:event|emit": return null;
      case "plugin:event|emit_to": return null;
      case "floaty_log": return null;
      case "floaty_heartbeat": return null;
      case "plugin:dialog|message": return "remove";
      case "plugin:dialog|open": return null;
      case "plugin:dialog|confirm": return true;
      default: return null;
    }
  };
  window.__CALLS__ = [];
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "settings" }, currentWebview: { label: "settings" } },
    transformCallback: (cb) => { const id = nextCb++; callbacks.set(id, cb); return id; },
    unregisterCallback: (id) => callbacks.delete(id),
    convertFileSrc: (p) => "http://asset.localhost/" + encodeURIComponent(p),
    invoke: async (cmd, args) => { window.__CALLS__.push(cmd); return answer(cmd, args); },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
})()`;

const MEASURE = `(() => {
  const shown = (el) => el && el.offsetParent !== null;
  return {
    hint: document.querySelector(".set-hint, .pane-hint")?.textContent?.trim() ?? null,
    groups: [...document.querySelectorAll(".set-group-title")].map((e) => e.textContent.trim()),
    rows: [...document.querySelectorAll(".set-label, .slider-label")].map((e) => e.textContent.trim()),
    choices: [...document.querySelectorAll("select")].map((s) => s.value),
    sliders: [...document.querySelectorAll("input[type=range]")].map((s) => s.value),
    buttons: [...document.querySelectorAll("button")].filter(shown).map((b) => (b.textContent || "").trim()),
    switches: document.querySelectorAll(".set-switch input").length,
    cards: document.querySelectorAll(".set-item").length,
    lists: document.querySelectorAll(".set-list").length,
    horizontalOverflow: document.documentElement.scrollWidth - window.innerWidth,
    calls: [...new Set(window.__CALLS__ ?? [])],
  };
})()`;

const results = [];

// The dev server has to answer before anything else is measured: pointed at a dead
// port, a probe renders blank panes and reports them as broken, when the truth is
// that there was nothing to look at. Exit 2 for that, and 1 for a page that drew
// wrong.
try {
  const alive = await fetch(base);
  if (!alive.ok) throw new Error(`HTTP ${alive.status}`);
} catch (err) {
  console.error(
    `verify: could not reach ${base} — is \`npm run dev\` running?\n  ${err.message}`,
  );
  process.exit(2);
}

// Nothing to look at is not the same as an app that is broken: exit 2 when the dev
// server (or Chrome) cannot be reached, so a failed probe never reads as a failed app.
let chrome;
try {
  chrome = await launchChrome({ inject: STUB, window: "500,900" });
} catch (err) {
  console.error(
    `verify: could not open ${base} — is \`npm run dev\` running?\n  ${err.message}`,
  );
  process.exit(2);
}
try {
  for (const pane of PANES) {
    // A query as well as the hash: `Page.navigate` between two urls that differ
    // only after `#` is a *same-document* navigation, so the pane would never be
    // rebuilt and the probe would measure the previous one.
    await chrome.session.send("Page.navigate", {
      url: `${base}/settings.html?pane=${pane}#/${pane}`,
    });
    await new Promise((r) => setTimeout(r, 1200));
    const measured = await chrome.session.evaluate(MEASURE);
    const errors = realErrors(chrome.session.errors);
    chrome.session.errors.length = 0;
    if (shiny) {
      await chrome.session.screenshot(path.join(here, "out", `pane-${pane}.png`));
    }
    results.push({ pane, ...measured, errors });
  }
} finally {
  chrome.stop();
}

let failed = 0;
for (const r of results) {
  const bad = r.errors.length > 0 || r.horizontalOverflow > 0 || r.groups.length === 0;
  if (bad) failed++;
  console.log(
    `${bad ? "FAIL" : "ok  "} ${r.pane.padEnd(8)} groups=${r.groups.length} cards=${r.cards} ` +
      `buttons=${r.buttons.length} rows=${r.rows.length} overflow=${r.horizontalOverflow}`,
  );
  if (r.errors.length) for (const e of r.errors) console.log(`       ${e}`);
  if (r.horizontalOverflow > 0) console.log(`       ${r.horizontalOverflow}px wider than the window`);
  if (r.groups.length === 0) console.log(`       drew nothing: ${JSON.stringify(r.calls)}`);
}
if (process.argv.includes("--json")) console.log(JSON.stringify(results, null, 2));
console.log(failed === 0 ? `\nall ${results.length} panes drew clean` : `\n${failed} pane(s) need a look`);
process.exit(failed === 0 ? 0 : 1);
