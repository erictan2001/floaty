/**
 * The settings window.
 *
 * It is a shell around one pane at a time: a fixed header, a row of tabs, and a
 * body that only the current tab fills. Everything the panes read lives in
 * `settings/store.ts`, and a pane never loads it itself — which is why
 * `changes()` can redraw the visible pane on any data change for free.
 *
 * The panes themselves are one file each under `settings/panes/`, and the row
 * builders they share are in `settings/dom.ts`.
 */

import { listen } from "@tauri-apps/api/event";
import { allPlugins, loadPlugins } from "./widgets/plugin";
import { crossCheckPlugins, manifestEntries } from "./widgets/pluginManifest";
import { startHeartbeat, watchSettings } from "./widgets/lib";
import { log, mountErrorBar, showError } from "./settings/api";
import { el } from "./settings/dom";
import { PANES } from "./settings/panes";
import { changes, loadSettings, reloadPlugins, reloadWidgets } from "./settings/store";
import "./style.css";

const root = document.getElementById("settings");
const tabs = new Map<string, HTMLButtonElement>();
let bodyHost: HTMLDivElement | undefined;
let active = "";

/** The pane the hash names, or the first one when it names nothing (or nonsense). */
function paneFromHash(): string {
  const id = window.location.hash.replace(/^#\/?/, "");
  return PANES.some((pane) => pane.id === id) ? id : PANES[0].id;
}

/** Draw one pane: its heading, then whatever the pane puts under it. */
function show(id: string): void {
  if (!bodyHost) return;
  const pane = PANES.find((p) => p.id === id) ?? PANES[0];
  active = pane.id;
  for (const [tabId, tab] of tabs) tab.classList.toggle("active", tabId === active);

  bodyHost.scrollTop = 0;
  bodyHost.innerHTML = "";
  const frame = el("div", "set-pane");
  const head = el("header", "set-pane-head");
  head.append(el("h1", "set-pane-title", pane.title));
  if (pane.hint) head.append(el("p", "set-pane-hint", pane.hint));
  const content = el("div", "set-pane-body");
  frame.append(head, content);
  bodyHost.append(frame);
  void pane.render(content);

  // The hash is the window's URL rather than history: a reload (and a plugin
  // rescan reloads both windows) comes back to the tab the user was on.
  if (window.location.hash !== `#/${pane.id}`) {
    window.history.replaceState(null, "", `#/${pane.id}`);
  }
}

function buildShell(): void {
  if (!root) {
    document.body.textContent = "settings root missing";
    throw new Error("settings root missing");
  }
  const shell = el("div", "set-shell");
  mountErrorBar(shell);

  const brand = el("div", "set-brand");
  brand.append(el("h1", "", "floaty"), el("p", "", "widgets living on your desktop"));
  // No close button of our own: the window keeps its default Windows frame, so
  // the title bar already has one and a second × under it is just a duplicate.
  const head = el("header", "set-head");
  head.append(brand);

  const nav = el("nav", "set-tabs");
  for (const pane of PANES) {
    const tab = el("button", "set-tab", pane.label);
    tab.type = "button";
    tab.addEventListener("click", () => show(pane.id));
    tabs.set(pane.id, tab);
    nav.append(tab);
  }

  bodyHost = el("div", "set-body");
  shell.append(head, nav, bodyHost);
  root.append(shell);
}

/** Load what the panes read from: the plugin registry, settings, floaties. */
async function boot(): Promise<void> {
  // The registry first: it fetches the manifest, and every pane asks the
  // manifest what a kind is — its name, its size, whether it is a file on disk.
  await loadPlugins();
  crossCheckPlugins(
    allPlugins()
      .map((plugin) => plugin.kind)
      .filter((kind) => manifestEntries().some((entry) => entry.id === kind && entry.source === "builtin")),
  );
  await loadSettings();
  await reloadPlugins();
  await reloadWidgets();
}

try {
  buildShell();
  show(paneFromHash());
  startHeartbeat("settings");
  // lib.ts keeps its own settings cache; `removeSelf` reads `confirm_remove`
  // from it, and the desktop pane removes through that same helper.
  watchSettings();
  window.addEventListener("hashchange", () => show(paneFromHash()));
  // Anything that changes the data redraws the pane that is up. Panes do not
  // subscribe themselves: only one of them is ever on screen.
  changes(() => show(active));
  listen("floaty-plugins-changed", () => {
    void reloadPlugins();
    void reloadWidgets();
  }).catch(() => undefined);
  // The pointed folder is watched, so the floatie list moves on its own: a file
  // added, renamed or deleted on disk shows up here without pressing sync.
  listen("floaty-widgets-changed", () => {
    void reloadWidgets();
  }).catch(() => undefined);
  void boot().catch((err) => {
    showError(`settings failed to load: ${err instanceof Error ? err.message : String(err)}`);
  });
} catch (err) {
  showError(`settings failed to start: ${err instanceof Error ? err.message : String(err)}`);
}

window.addEventListener("error", (e) => log(`[settings] error: ${e.message} @${e.lineno}:${e.colno}`));
window.addEventListener("unhandledrejection", (e) =>
  log(`[settings] rejection: ${String((e as PromiseRejectionEvent).reason)}`),
);
