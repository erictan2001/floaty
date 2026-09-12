import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { pluginFor, plugins, type PluginSettingsContext } from "./widgets/plugin";
import type { FloatSettings } from "./widgets/lib";
import "./style.css";

interface WidgetRecord {
  id: string;
  kind: string;
  x: number;
  y: number;
  data: Record<string, unknown>;
}

interface DiscoveredApp {
  name: string;
  path: string;
}

const root = document.getElementById("settings");
const appWin = getCurrentWindow();
let appsCache: DiscoveredApp[] = [];
let filter = "";
let scanning = false;
let refreshSeq = 0;
// shared control rows, built once below and moved into plugin cards
const sliderRows = new Map<string, HTMLElement>();
const selectRows = new Map<string, HTMLElement>();
// plugin kinds currently disabled (drives add-row + desktop list filtering)
const disabledSet = new Set<string>();

let latestSettings: FloatSettings = {
  pet_speed: 1,
  gravity: 2600,
  bounce: 0.45,
  floatiness: 1,
  single_click: "nothing",
  double_click: "launch",
  live2d_root: "",
  files_root: "",
  disabled: [],
  stay_on_desktop: true,
  animated_ratio: 100,
  animation_mode: "wave",
};

if (!root) {
  document.body.textContent = "settings root missing";
  throw new Error("settings root missing");
}

function showError(msg: string): void {
  let bar = document.getElementById("settings-error");
  if (!bar) {
    bar = document.createElement("div");
    bar.id = "settings-error";
    bar.className = "error-bar";
    document.body.prepend(bar);
  }
  bar.textContent = msg;
  bar.style.display = "block";
  window.clearTimeout((showError as unknown as { t?: number }).t);
  (showError as unknown as { t?: number }).t = window.setTimeout(() => {
    bar!.style.display = "none";
  }, 6000);
}

async function safe<T>(label: string, fn: () => Promise<T>): Promise<T | undefined> {
  // fire-and-forget markers: if an invoke never settles, the start line with
  // no matching end line in floaty.log names the stuck call.
  invoke("floaty_log", { msg: `[settings] invoke-start: ${label}` }).catch(() => undefined);
  try {
    const r = await fn();
    invoke("floaty_log", { msg: `[settings] invoke-end: ${label}` }).catch(() => undefined);
    return r;
  } catch (e) {
    invoke("floaty_log", { msg: `[settings] invoke-error: ${label}` }).catch(() => undefined);
    showError(`${label}: ${e instanceof Error ? e.message : String(e)}`);
    return undefined;
  }
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
}

function sectionTitle(box: HTMLElement, text: string): void {
  box.append(el("h2", "sec-title", text));
}

async function refreshWidgets(box: HTMLElement): Promise<void> {
  const host = document.getElementById("widget-list");
  if (!host) return;
  const seq = ++refreshSeq;
  host.innerHTML = "";
  const list = await safe("load widgets", () => invoke<WidgetRecord[]>("floaty_list"));
  if (seq !== refreshSeq) return; // a newer refresh superseded this one
  if (!list) {
    host.append(el("div", "empty", "couldn't load widgets"));
    return;
  }
  const visible = list.filter((w) => !disabledSet.has(w.kind));
  if (visible.length === 0) {
    host.append(el("div", "empty", "no floaties yet — add one above, or use the tray icon"));
    return;
  }
  for (const w of [...visible].sort((a, b) => a.id.localeCompare(b.id))) {
    const card = el("div", "card");
    const kind = el("span", `kind k-${w.kind}`, w.kind);
    const ident = el("span", "id", w.id);
    const plugin = pluginFor(w.kind);
    const detail = plugin?.describe(w);
    ident.textContent = detail ? `${w.id} — ${detail}` : w.id;
    const del = el("button", "danger", "remove");
    del.addEventListener("click", async () => {
      if ((del as HTMLButtonElement).disabled) return;
      (del as HTMLButtonElement).disabled = true;
      try {
        await safe("remove widget", () => invoke("floaty_remove", { id: w.id }));
        await refreshWidgets(box);
      } finally {
        (del as HTMLButtonElement).disabled = false;
      }
    });
    card.append(kind, ident, del);
    if (plugin?.renderWidgetControls) {
      plugin.renderWidgetControls(
        card,
        w,
        {
          safe,
          refreshWidgets: () => refreshWidgets(box),
          showError,
        },
        del,
      );
    }
    host.append(card);
  }
  void box;
}

function renderAppResults(): void {
  const host = document.getElementById("app-results");
  const count = document.getElementById("app-count");
  if (!host) return;
  host.innerHTML = "";
  const q = filter.trim().toLowerCase();
  const matches = q
    ? appsCache.filter((a) => a.name.toLowerCase().includes(q))
    : appsCache;
  if (count) {
    count.textContent =
      appsCache.length === 0
        ? "press scan to find your apps"
        : `${matches.length} of ${appsCache.length} apps`;
  }
  if (appsCache.length === 0) {
    host.append(el("div", "empty", "nothing scanned yet"));
    return;
  }
  if (matches.length === 0) {
    host.append(el("div", "empty", "no matches"));
    return;
  }
  for (const a of matches.slice(0, 60)) {
    const card = el("div", "card");
    const dot = el("span", "app-dot", (a.name.trim()[0] ?? "?").toUpperCase());
    const nm = el("span", "id", a.name);
    nm.title = a.path;
    const btn = el("button", "pill small", "float it");
    btn.addEventListener("click", async () => {
      if ((btn as HTMLButtonElement).disabled) return;
      (btn as HTMLButtonElement).disabled = true;
      btn.textContent = "floating...";
      try {
        const rec = await safe("add launcher", () =>
          invoke<WidgetRecord>("floaty_add_launcher", { name: a.name, path: a.path }),
        );
        if (rec) {
          btn.textContent = "floated!";
          window.setTimeout(() => (btn.textContent = "float it"), 1200);
        } else {
          btn.textContent = "float it";
        }
      } finally {
        (btn as HTMLButtonElement).disabled = false;
      }
    });
    card.append(dot, nm, btn);
    host.append(card);
  }
  if (matches.length > 60) {
    host.append(el("div", "empty", `…and ${matches.length - 60} more — refine the search`));
  }
}

async function scan(box: HTMLElement): Promise<void> {
  if (scanning) return;
  scanning = true;
  const btn = document.getElementById("scan-btn") as HTMLButtonElement | null;
  if (btn) {
    btn.textContent = "scanning…";
    btn.disabled = true;
  }
  const found = await safe("scan apps", () => invoke<DiscoveredApp[]>("floaty_scan_apps"));
  if (found) appsCache = found;
  scanning = false;
  if (btn) {
    btn.textContent = "scan apps";
    btn.disabled = false;
  }
  renderAppResults();
  void box;
}

function renderAddRow(box: HTMLElement): void {
  const host = document.getElementById("add-row");
  if (!host) return;
  host.innerHTML = "";
  for (const p of plugins) {
    if (!p.addLabel || disabledSet.has(p.kind)) continue;
    const b = el("button", "pill", p.addLabel);
    b.addEventListener("click", async () => {
      if ((b as HTMLButtonElement).disabled) return;
      (b as HTMLButtonElement).disabled = true;
      try {
        const rec = await safe("add widget", () => invoke<WidgetRecord>("floaty_create", { kind: p.kind }));
        if (rec) await refreshWidgets(box);
      } finally {
        (b as HTMLButtonElement).disabled = false;
      }
    });
    host.append(b);
  }
  if (!host.hasChildNodes()) {
    host.append(el("div", "empty", "all plugins disabled"));
  }
}

function build(): void {
  if (!root) return;
  root.innerHTML = "";
  const box = el("div", "settings");

  box.append(el("h1", "", "floaty"));
  const sub = el("p", "sub", "widgets living on your desktop");
  box.append(sub);

  // quick add (buttons rendered per enabled plugin by the manager)
  sectionTitle(box, "new floatie");
  const addRow = el("div", "add-row");
  addRow.id = "add-row";
  box.append(addRow);

  // app launchers
  sectionTitle(box, "app floaties");
  const hint = el("p", "hint", "drop real apps onto your desktop — they fall, bounce and pile up. double-click to open.");
  box.append(hint);
  const scanRow = el("div", "scan-row");
  const search = el("input", "search");
  search.placeholder = "search apps…";
  search.addEventListener("input", () => {
    filter = (search as HTMLInputElement).value;
    renderAppResults();
  });
  const scanBtn = el("button", "pill ghost", "scan apps");
  scanBtn.id = "scan-btn";
  scanBtn.addEventListener("click", () => void scan(box));
  scanRow.append(search, scanBtn);
  box.append(scanRow);
  const count = el("div", "count", "");
  count.id = "app-count";
  box.append(count);
  const results = el("div", "", "");
  results.id = "app-results";
  box.append(results);

  // files & folders root directory
  sectionTitle(box, "files & folders");
  const filesHint = el(
    "p",
    "hint",
    "Point to a root folder to float its files and directories on your desktop. Dragging an item out of a folder moves it on disk to the desktop directory.",
  );
  box.append(filesHint);
  const filesRow = el("div", "slider-row");
  filesRow.append(el("span", "slider-label", "root dir"));
  const filesPath = el("span", "folder-path", latestSettings.files_root || "no folder set");
  filesPath.id = "files-root";
  filesPath.title = latestSettings.files_root || "";

  const filesBrowse = el("button", "pill small", "browse");
  const filesSync = el("button", "pill small ghost", "sync");
  const filesCount = el("div", "count", "");
  filesCount.id = "files-count";

  const runFilesSync = async (rootPath?: string): Promise<void> => {
    const target = rootPath ?? latestSettings.files_root;
    if (!target) {
      filesCount.textContent = "no root folder set";
      return;
    }
    filesSync.disabled = true;
    filesSync.textContent = "syncing…";
    try {
      const res = await safe("sync files", () =>
        invoke<{ files: number; dirs: number; total: number }>("floaty_sync_files", { root: target }),
      );
      if (res) {
        filesCount.textContent = `${res.files} file${res.files === 1 ? "" : "s"}, ${res.dirs} folder${res.dirs === 1 ? "" : "s"} synced to desktop`;
        try {
          await refreshWidgets(box);
        } catch (e) {
          console.error("refreshWidgets error:", e);
        }
      }
    } finally {
      filesSync.disabled = false;
      filesSync.textContent = "sync";
    }
  };

  filesBrowse.addEventListener("click", async () => {
    if (filesBrowse.disabled) return;
    filesBrowse.disabled = true;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked === "string" && picked) {
        latestSettings.files_root = picked;
        filesPath.textContent = picked;
        filesPath.title = picked;
        await updateSettings({ files_root: picked });
        await runFilesSync(picked);
      }
    } catch {
      showError("folder picker failed");
    } finally {
      filesBrowse.disabled = false;
    }
  });

  filesSync.addEventListener("click", () => void runFilesSync());

  filesRow.append(filesPath, filesBrowse, filesSync);
  box.append(filesRow, filesCount);

  // floating parameters + icon click behaviour live in the plugin cards
  // below; the shared rows are built here and moved into those cards
  const sliderDefs: Array<{
    key: string;
    label: string;
    min: number;
    max: number;
    step: number;
    unit?: string;
  }> = [
    { key: "pet_speed", label: "pet speed", min: 0, max: 2, step: 0.1 },
    { key: "gravity", label: "gravity", min: 0, max: 5000, step: 50 },
    { key: "bounce", label: "bounce", min: 0, max: 0.9, step: 0.05 },
    { key: "floatiness", label: "float", min: 0, max: 2, step: 0.1 },
    { key: "animated_ratio", label: "animated icons", min: 0, max: 100, step: 10, unit: "%" },
  ];
  const sliderInputs = new Map<string, HTMLInputElement>();
  const sliderVals = new Map<string, HTMLElement>();
  const clickSelects = new Map<string, HTMLSelectElement>();
  let stayOnDesktop = true;
  let stayOnDesktopInput: HTMLInputElement | undefined;
  let saveTimer: number | undefined;
  async function saveFloating(): Promise<void> {
    window.clearTimeout(saveTimer);
    await new Promise<void>((resolve) => {
      saveTimer = window.setTimeout(() => resolve(), 250);
    });
    const num = (k: string, fb: number): number => {
      const v = Number(sliderInputs.get(k)?.value);
      return Number.isFinite(v) ? v : fb;
    };
    latestSettings = {
      ...latestSettings,
      pet_speed: num("pet_speed", 1),
      gravity: num("gravity", 2600),
      bounce: num("bounce", 0.45),
      floatiness: num("floatiness", 1),
      animated_ratio: num("animated_ratio", 100),
      single_click: (clickSelects.get("single_click")?.value as FloatSettings["single_click"]) ?? "drop",
      double_click: (clickSelects.get("double_click")?.value as FloatSettings["double_click"]) ?? "launch",
      animation_mode: clickSelects.get("animation_mode")?.value ?? "wave",
      stay_on_desktop: stayOnDesktopInput ? stayOnDesktopInput.checked : stayOnDesktop,
    };
    await safe("save settings", () =>
      invoke("floaty_set_settings", { settings: latestSettings }),
    );
  }

  async function updateSettings(patch: Partial<FloatSettings>): Promise<void> {
    latestSettings = { ...latestSettings, ...patch };
    await safe("save settings", () =>
      invoke("floaty_set_settings", { settings: latestSettings }),
    );
  }

  for (const d of sliderDefs) {
    const row = el("div", "slider-row");
    row.append(el("span", "slider-label", d.label));
    const input = el("input", "slider");
    input.type = "range";
    input.min = String(d.min);
    input.max = String(d.max);
    input.step = String(d.step);
    const val = el("span", "slider-val", "");
    input.addEventListener("input", () => {
      val.textContent = d.unit ? `${input.value}${d.unit}` : input.value;
      void saveFloating();
    });
    sliderInputs.set(d.key, input);
    sliderVals.set(d.key, val);
    sliderRows.set(d.key, row);
    row.append(input, val);
  }

  const clickDefs: Array<{ key: string; label: string; opts: string[] }> = [
    { key: "single_click", label: "single click", opts: ["drop", "hop", "nothing"] },
    { key: "double_click", label: "double click", opts: ["launch", "drop", "nothing"] },
    { key: "animation_mode", label: "animation mode", opts: ["wave", "sync", "gentle", "static"] },
  ];
  for (const d of clickDefs) {
    const row = el("div", "slider-row");
    row.append(el("span", "slider-label", d.label));
    const sel = el("select", "select");
    for (const o of d.opts) {
      const opt = document.createElement("option");
      opt.value = o;
      opt.textContent = o;
      sel.append(opt);
    }
    sel.addEventListener("change", () => void saveFloating());
    clickSelects.set(d.key, sel);
    selectRows.set(d.key, row);
    row.append(sel);
  }
  void (async () => {
    const s = await safe("load settings", () =>
      invoke<FloatSettings>("floaty_get_settings"),
    );
    if (!s) return;
    latestSettings = { ...latestSettings, ...s };
    for (const d of sliderDefs) {
      const v = s[d.key as keyof FloatSettings];
      if (typeof v === "number") {
        sliderInputs.get(d.key)!.value = String(v);
        sliderVals.get(d.key)!.textContent = d.unit ? `${v}${d.unit}` : String(v);
      }
    }
    for (const d of clickDefs) {
      const v = s[d.key as keyof FloatSettings];
      if (typeof v === "string") clickSelects.get(d.key)!.value = v;
    }
    if (typeof s.stay_on_desktop === "boolean") {
      stayOnDesktop = s.stay_on_desktop;
      if (stayOnDesktopInput) {
        stayOnDesktopInput.checked = stayOnDesktop;
      }
    }
    if (s.files_root) {
      filesPath.textContent = s.files_root;
      filesPath.title = s.files_root;
    }
    void loadManager();
  })();

  // current widgets
  sectionTitle(box, "on your desktop");
  const list = el("div", "", "");
  list.id = "widget-list";
  box.append(list);

  // plugin manager: enable toggles + per-plugin parameters
  sectionTitle(box, "plugins");
  const pluginHost = el("div", "");
  pluginHost.id = "plugin-list";
  box.append(pluginHost);

  interface PluginState {
    id: string;
    name: string;
    description: string;
    enabled: boolean;
  }

  async function loadManager(): Promise<void> {
    const list = await safe("load plugins", () => invoke<PluginState[]>("floaty_plugins"));
    if (!list) return;
    disabledSet.clear();
    for (const p of list) if (!p.enabled) disabledSet.add(p.id);
    pluginHost.innerHTML = "";
    for (const p of list) {
      const card = el("div", "card plugin-card");
      const head = el("div", "plugin-head");
      head.append(el("span", `kind k-${p.id}`, p.id), el("span", "id", p.name));
      const tog = el("label", "plugin-toggle");
      const chk = el("input", "");
      chk.type = "checkbox";
      chk.checked = p.enabled;
      chk.addEventListener("change", async () => {
        chk.disabled = true;
        await safe("toggle plugin", () =>
          invoke("floaty_set_plugin_enabled", { id: p.id, enabled: chk.checked }),
        );
        chk.disabled = false;
        void loadManager();
      });
      tog.append(chk, el("span", "", "enabled"));
      head.append(tog);
      card.append(head);
      card.append(el("p", "hint", p.description));

      const plugin = pluginFor(p.id);
      if (plugin?.renderSettings) {
        const settingsCtx: PluginSettingsContext = {
          getSettings: () => latestSettings,
          getSharedRow: (name: string) => sliderRows.get(name) ?? selectRows.get(name),
          safe,
          showError,
          refreshWidgets: () => refreshWidgets(box),
          updateSettings,
        };
        await plugin.renderSettings(card, settingsCtx);
      }
      pluginHost.append(card);
    }
    renderAddRow(box);
    await refreshWidgets(box);
  }
  import("@tauri-apps/api/event")
    .then((m) => m.listen("floaty-plugins-changed", () => void loadManager()))
    .catch(() => undefined);

  // desktop behavior
  sectionTitle(box, "desktop behavior");
  const desktopCard = el("div", "card plugin-card");
  const dHead = el("div", "plugin-head");
  dHead.append(el("span", "kind k-app", "desktop"), el("span", "id", "Stay on desktop (Win+D)"));
  const dTog = el("label", "plugin-toggle");
  stayOnDesktopInput = el("input", "");
  stayOnDesktopInput.type = "checkbox";
  stayOnDesktopInput.checked = stayOnDesktop;
  stayOnDesktopInput.addEventListener("change", () => {
    stayOnDesktop = stayOnDesktopInput!.checked;
    void saveFloating();
  });
  dTog.append(stayOnDesktopInput, el("span", "", "enabled"));
  dHead.append(dTog);
  desktopCard.append(dHead);
  desktopCard.append(
    el(
      "p",
      "hint",
      "Keep floaties visible on screen when Show Desktop (Win+D) is invoked. Turn off to minimize them with other apps.",
    ),
  );
  box.append(desktopCard);

  // footer
  const foot = el("div", "foot-row");
  const hide = el("button", "pill ghost", "close settings");
  hide.addEventListener("click", () =>
    safe("close", () => appWin.hide()).then(() => undefined),
  );
  const quit = el("button", "pill danger-btn", "quit floaty");
  quit.addEventListener("click", () =>
    safe("quit", () => invoke("floaty_quit")).then(() => undefined),
  );
  foot.append(hide, quit);
  box.append(foot);

  root.append(box);
  void scan(box);
  void loadManager();
}

try {
  build();
} catch (e) {
  showError(`settings failed to start: ${e instanceof Error ? e.message : String(e)}`);
}

window.addEventListener("error", (e) => {
  invoke("floaty_log", { msg: `[settings] error: ${e.message} @${e.lineno}:${e.colno}` }).catch(
    () => undefined,
  );
});
window.addEventListener("unhandledrejection", (e) => {
  invoke("floaty_log", {
    msg: `[settings] rejection: ${String((e as PromiseRejectionEvent).reason)}`,
  }).catch(() => undefined);
});
