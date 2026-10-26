/**
 * The settings window.
 *
 * It is a rail of sections and one pane at a time beside it. Everything the
 * panes read lives in `settings/store.ts`, and a pane never loads it itself —
 * which is why `changes()` can redraw the visible pane on any data change.
 *
 * A redraw keeps the scroll position and waits while a field has focus: a
 * settings save that the backend clamps, or a file watcher event, used to
 * rebuild the pane under the user's pointer and jump the window to the top.
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
import { PANE_GROUPS, PANES } from "./settings/panes";
import { changes, loadSettings, reloadPlugins, reloadWidgets } from "./settings/store";
import "./style.css";

const root = document.getElementById("settings");
const tabs = new Map<string, HTMLButtonElement>();
let bodyHost: HTMLDivElement | undefined;
let active = "";
let redrawTimer: number | undefined;

/** The pane the hash names, or the first one when it names nothing (or nonsense). */
function paneFromHash(): string {
  const id = window.location.hash.replace(/^#\/?/, "");
  return PANES.some((pane) => pane.id === id) ? id : PANES[0].id;
}

/** Draw one pane: its heading, then whatever the pane puts under it. */
function show(id: string, keepScroll = false): void {
  if (!bodyHost) return;
  const pane = PANES.find((p) => p.id === id) ?? PANES[0];
  active = pane.id;
  const scroll = keepScroll ? bodyHost.scrollTop : 0;
  for (const [tabId, tab] of tabs) {
    const on = tabId === active;
    tab.classList.toggle("active", on);
    if (on) tab.setAttribute("aria-current", "page");
    else tab.removeAttribute("aria-current");
  }

  bodyHost.innerHTML = "";
  const frame = el("div", "set-pane");
  const head = el("header", "set-pane-head");
  head.append(el("h1", "set-pane-title", pane.title));
  if (pane.hint) head.append(el("p", "set-pane-hint", pane.hint));
  const content = el("div", "set-pane-body");
  frame.append(head, content);
  bodyHost.append(frame);
  bodyHost.scrollTop = scroll;

  // Draw now, then put the scroll back when the pane is really done: several
  // panes fill their list in a promise, and until it lands the body is short
  // enough that the browser clamps the restored position to the top.
  void (async () => {
    try {
      await pane.render(content);
    } catch (err) {
      showError(
        `could not draw ${pane.label}: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      if (active === pane.id) bodyHost!.scrollTop = scroll;
    }
  })();

  // The hash is the window's URL rather than history: a reload (and a plugin
  // rescan reloads both windows) comes back to the pane the user was on.
  if (window.location.hash !== `#/${pane.id}`) {
    window.history.replaceState(null, "", `#/${pane.id}`);
  }
}

/**
 * Is the user in the middle of editing a value?
 *
 * Only fields in the pane count: focus on a nav button is not editing, and a
 * redraw may go ahead.
 */
function isEditing(): boolean {
  const activeEl = document.activeElement;
  return (
    activeEl instanceof HTMLElement &&
    activeEl.closest(".set-pane") !== null &&
    (activeEl.matches("input, select, textarea") || activeEl.closest(".set-switch") !== null)
  );
}

/**
 * Redraw only when the user is not holding a control.
 *
 * A clamped save or a watcher event can land while a slider is being dragged or
 * a value is being typed; replacing the pane would drop the pointer and jump the
 * scroll. Wait for the control to lose focus, then draw once. The check repeats
 * rather than listening for `focusout` alone: a field can lose focus without
 * the event reaching the page, and a redraw that never lands is stale.
 */
function scheduleRedraw(): void {
  window.clearTimeout(redrawTimer);
  redrawTimer = window.setTimeout(() => {
    if (isEditing()) {
      scheduleRedraw();
      return;
    }
    show(active, true);
  }, 200);
}

function buildShell(): void {
  if (!root) {
    document.body.textContent = "settings root missing";
    throw new Error("settings root missing");
  }
  const shell = el("div", "set-shell");
  mountErrorBar(shell);

  const rail = el("aside", "set-rail");
  const brand = el("div", "set-brand");
  brand.append(el("h1", "", "floaty"), el("p", "", "settings"));

  const nav = el("nav", "set-nav");
  nav.setAttribute("aria-label", "Settings sections");
  for (const group of PANE_GROUPS) {
    const section = el("div", "set-nav-group");
    section.append(el("div", "set-nav-label", group.label));
    for (const pane of group.panes) {
      const tab = el("button", "set-tab", pane.label);
      tab.type = "button";
      tab.addEventListener("click", () => {
        window.clearTimeout(redrawTimer);
        // Clicking the section already open refreshes it without jumping to the top.
        show(pane.id, pane.id === active);
      });
      tabs.set(pane.id, tab);
      section.append(tab);
    }
    nav.append(section);
  }
  rail.append(brand, nav);

  const main = el("main", "set-main");
  bodyHost = el("div", "set-body");
  main.append(bodyHost);
  shell.append(rail, main);
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
      .filter((kind) =>
        manifestEntries().some((entry) => entry.id === kind && entry.source === "builtin"),
      ),
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
  window.addEventListener("hashchange", () => {
    window.clearTimeout(redrawTimer);
    show(paneFromHash());
  });
  // Anything that changes the data redraws the pane that is up. Panes do not
  // subscribe themselves: only one of them is ever on screen. The scroll and
  // the control under the pointer are kept: this is a refresh, not navigation.
  changes(() => {
    if (isEditing()) {
      scheduleRedraw();
      return;
    }
    window.clearTimeout(redrawTimer);
    show(active, true);
  });
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

window.addEventListener("error", (e) =>
  log(`[settings] error: ${e.message} @${e.lineno}:${e.colno}`),
);
window.addEventListener("unhandledrejection", (e) =>
  log(`[settings] rejection: ${String((e as PromiseRejectionEvent).reason)}`),
);
