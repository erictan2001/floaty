import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
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
  if (list.length === 0) {
    host.append(el("div", "empty", "no floaties yet — add one above, or use the tray icon"));
    return;
  }
  for (const w of [...list].sort((a, b) => a.id.localeCompare(b.id))) {
    const card = el("div", "card");
    const kind = el("span", `kind k-${w.kind}`, w.kind);
    const ident = el("span", "id", w.id);
    if (w.kind === "app" && typeof w.data["name"] === "string") {
      ident.textContent = `${w.id} — ${w.data["name"] as string}`;
    } else if (w.kind === "note" && typeof w.data["text"] === "string") {
      ident.textContent = `${w.id} — ${(w.data["text"] as string).slice(0, 32)}`;
    }
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

function build(): void {
  if (!root) return;
  root.innerHTML = "";
  const box = el("div", "settings");

  box.append(el("h1", "", "floaty"));
  const sub = el("p", "sub", "widgets living on your desktop");
  box.append(sub);

  // quick add
  sectionTitle(box, "new floatie");
  const addRow = el("div", "add-row");
  const defs: Array<[string, string]> = [
    ["note", "+ note"],
    ["clock", "+ clock"],
    ["pet", "+ pet"],
  ];
  for (const [kind, label] of defs) {
    const b = el("button", "pill", label);
    b.addEventListener("click", async () => {
      if ((b as HTMLButtonElement).disabled) return;
      (b as HTMLButtonElement).disabled = true;
      try {
        const rec = await safe("add widget", () => invoke<WidgetRecord>("floaty_create", { kind }));
        if (rec) await refreshWidgets(box);
      } finally {
        (b as HTMLButtonElement).disabled = false;
      }
    });
    addRow.append(b);
  }
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

  // floating parameters + icon click behaviour (persisted backend-side,
  // broadcast live to all widget windows)
  sectionTitle(box, "floating");
  const sliderDefs: Array<{ key: string; label: string; min: number; max: number; step: number }> = [
    { key: "pet_speed", label: "pet speed", min: 0, max: 2, step: 0.1 },
    { key: "gravity", label: "gravity", min: 0, max: 5000, step: 50 },
    { key: "bounce", label: "bounce", min: 0, max: 0.9, step: 0.05 },
  ];
  const sliderInputs = new Map<string, HTMLInputElement>();
  const sliderVals = new Map<string, HTMLElement>();
  const clickSelects = new Map<string, HTMLSelectElement>();
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
    await safe("save settings", () =>
      invoke("floaty_set_settings", {
        settings: {
          pet_speed: num("pet_speed", 1),
          gravity: num("gravity", 2600),
          bounce: num("bounce", 0.45),
          single_click: clickSelects.get("single_click")?.value ?? "drop",
          double_click: clickSelects.get("double_click")?.value ?? "launch",
        },
      }),
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
      val.textContent = input.value;
      void saveFloating();
    });
    sliderInputs.set(d.key, input);
    sliderVals.set(d.key, val);
    row.append(input, val);
    box.append(row);
  }

  sectionTitle(box, "icon clicks");
  const clickDefs: Array<{ key: string; label: string; opts: string[] }> = [
    { key: "single_click", label: "single click", opts: ["drop", "hop", "nothing"] },
    { key: "double_click", label: "double click", opts: ["launch", "drop", "nothing"] },
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
    row.append(sel);
    box.append(row);
  }
  void (async () => {
    const s = await safe("load settings", () =>
      invoke<Record<string, unknown>>("floaty_get_settings"),
    );
    if (!s) return;
    for (const d of sliderDefs) {
      const v = s[d.key];
      if (typeof v === "number") {
        sliderInputs.get(d.key)!.value = String(v);
        sliderVals.get(d.key)!.textContent = String(v);
      }
    }
    for (const d of clickDefs) {
      const v = s[d.key];
      if (typeof v === "string") clickSelects.get(d.key)!.value = v;
    }
  })();

  // current widgets
  sectionTitle(box, "on your desktop");
  const list = el("div", "", "");
  list.id = "widget-list";
  box.append(list);

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
  void refreshWidgets(box);
  void scan(box);
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
