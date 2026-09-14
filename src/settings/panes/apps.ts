/**
 * The Apps tab: the shortcuts found on this machine, searchable, one press to
 * float one. The scan is the expensive part of the old page — it walks the Start
 * Menu — so it runs when this tab is first opened rather than on every settings
 * launch, and its results scroll inside the pane instead of pushing everything
 * else down the page.
 */

import { invoke } from "@tauri-apps/api/core";
import { safe } from "../api";
import { action, actionRow, card, el, emptyState, group, note } from "../dom";
import type { Pane } from "../pane";
import { addLauncher, reloadWidgets } from "../store";

interface DiscoveredApp {
  name: string;
  path: string;
}

/** How many matches are drawn: more than this and the search is the tool. */
const CAP = 80;

let host: HTMLElement | undefined;
let results: HTMLElement | undefined;
let countLine: HTMLElement | undefined;
let found: DiscoveredApp[] | null = null;
let filter = "";
let scanning = false;

function renderResults(): void {
  if (!results || !countLine) return;
  results.innerHTML = "";
  const list = found ?? [];
  const query = filter.trim().toLowerCase();
  const matches = query ? list.filter((app) => app.name.toLowerCase().includes(query)) : list;
  countLine.textContent =
    list.length === 0 ? "press scan to find your apps" : `${matches.length} of ${list.length} apps`;

  if (list.length === 0) {
    results.append(emptyState("nothing scanned yet"));
    return;
  }
  if (matches.length === 0) {
    results.append(emptyState("no matches"));
    return;
  }
  for (const app of matches.slice(0, CAP)) results.append(appCard(app));
  if (matches.length > CAP) {
    results.append(note(`…and ${matches.length - CAP} more — refine the search`));
  }
}

function appCard(app: DiscoveredApp): HTMLElement {
  const item = card();
  item.append(el("span", "app-dot", (app.name.trim()[0] ?? "?").toUpperCase()));
  const name = el("span", "set-name", app.name);
  name.title = app.path;

  const floatBtn = el("button", "pill small", "float it");
  floatBtn.addEventListener("click", async () => {
    if (floatBtn.disabled) return;
    floatBtn.disabled = true;
    const rec = await addLauncher(app.name, app.path);
    if (!rec) {
      floatBtn.disabled = false;
      return;
    }
    floatBtn.textContent = "floated!";
    await reloadWidgets();
    window.setTimeout(() => {
      floatBtn.textContent = "float it";
      floatBtn.disabled = false;
    }, 1200);
  });
  item.append(name, floatBtn);
  return item;
}

async function scan(): Promise<void> {
  if (scanning) return;
  scanning = true;
  const list = await safe("scan apps", () => invoke<DiscoveredApp[]>("floaty_scan_apps"));
  scanning = false;
  if (list) found = list;
  renderResults();
}

function draw(): void {
  if (!host || !host.isConnected) return;
  host.innerHTML = "";

  const search = el("input", "search");
  search.placeholder = "search apps…";
  search.value = filter;
  search.addEventListener("input", () => {
    filter = search.value;
    renderResults();
  });

  const section = group("Find an app");
  const controls = actionRow();
  controls.append(search, action("scan apps", () => scan(), { busyLabel: "scanning…" }));
  countLine = el("div", "count", "");
  results = el("div", "set-scroll");
  section.body.append(controls, countLine, results);
  host.append(section.root);

  renderResults();
  // The list is the whole point of the tab, so fill it on the first visit.
  if (found === null && !scanning) void scan();
}

export const appsPane: Pane = {
  id: "apps",
  label: "Apps",
  title: "Apps",
  hint: "Float a real app: it drops onto the desktop, falls and bounces like the icons do, and opens on double-click.",
  render(node) {
    host = node;
    draw();
  },
};
