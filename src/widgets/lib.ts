import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { currentMonitor, getCurrentWindow, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";

export interface WidgetRecord {
  id: string;
  kind: string;
  /** logical px — matches Rust builder + store */
  x: number;
  y: number;
  data: Record<string, unknown>;
}

export const appWin = getCurrentWindow();

async function scaleFactor(): Promise<number> {
  try {
    const s = await appWin.scaleFactor();
    return s > 0 ? s : 1;
  } catch {
    return 1;
  }
}

/** Window position in logical px. */
export async function logicalPos(): Promise<{ x: number; y: number }> {
  const s = await scaleFactor();
  const p = await appWin.outerPosition();
  return { x: p.x / s, y: p.y / s };
}

export async function setLogicalPos(x: number, y: number): Promise<void> {
  const s = await scaleFactor();
  await appWin.setPosition(new PhysicalPosition(Math.round(x * s), Math.round(y * s)));
}

export interface MonitorArea {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Current monitor work area in logical px. */
export async function monitorArea(): Promise<MonitorArea> {
  const s = await scaleFactor();
  try {
    const m = await currentMonitor();
    if (m) {
      return {
        x: m.position.x / s,
        y: m.position.y / s,
        w: m.size.width / s,
        h: m.size.height / s,
      };
    }
  } catch {
    /* fall through */
  }
  return { x: 0, y: 0, w: 1280, h: 800 };
}

export async function loadRecord(id: string): Promise<WidgetRecord | undefined> {
  const list = await invoke<WidgetRecord[]>("floaty_list");
  return list.find((r) => r.id === id);
}

export async function saveRecord(rec: WidgetRecord): Promise<void> {
  await invoke("floaty_save", { record: rec });
}

export function debounce<F extends (...args: never[]) => void>(
  fn: F,
  ms: number,
): (...args: Parameters<F>) => void {
  let t: number | undefined;
  return (...args: Parameters<F>) => {
    window.clearTimeout(t);
    t = window.setTimeout(() => fn(...args), ms);
  };
}

/** Persist window position (logical px) every 2s + on unload. */
export function trackPosition(rec: WidgetRecord): void {
  const snap = async () => {
    try {
      const p = await logicalPos();
      const x = Math.round(p.x);
      const y = Math.round(p.y);
      if (x !== rec.x || y !== rec.y) {
        rec.x = x;
        rec.y = y;
        await saveRecord(rec);
      }
    } catch {
      /* window gone */
    }
  };
  window.setInterval(() => void snap(), 2000);
  window.addEventListener("beforeunload", () => void snap());
  // persist the moment a drag ends or the window hides instead of waiting
  // for the next poll — quits otherwise lose the last move
  window.addEventListener("pointerup", () => window.setTimeout(() => void snap(), 60));
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) void snap();
  });
}

/** Title bar that native-drags the window. Buttons inside must stopPropagation. */
export function makeBar(title: string, onClose: () => void): HTMLElement {
  const bar = document.createElement("div");
  bar.className = "bar";
  const dots = document.createElement("span");
  dots.className = "dots";
  const label = document.createElement("span");
  label.className = "bar-title";
  label.textContent = title;
  const close = document.createElement("button");
  close.className = "icon-btn";
  close.textContent = "\u00d7";
  close.title = "Remove widget";
  close.addEventListener("pointerdown", (e) => e.stopPropagation());
  close.addEventListener("click", (e) => {
    e.stopPropagation();
    onClose();
  });
  bar.append(dots, label, close);
  bar.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    void appWin.startDragging().catch(() => undefined);
  });
  return bar;
}

export async function removeSelf(rec: WidgetRecord): Promise<void> {
  try {
    await invoke("floaty_remove", { id: rec.id });
  } catch {
    await appWin.close().catch(() => undefined);
  }
}

// ---------- global floating settings (cached, live-updated) ----------

export interface FloatSettings {
  pet_speed: number;
  gravity: number;
  bounce: number;
  floatiness: number;
  single_click: string;
  double_click: string;
  live2d_root: string;
  disabled: string[];
}

const DEFAULT_SETTINGS: FloatSettings = {
  pet_speed: 1,
  gravity: 2600,
  bounce: 0.45,
  floatiness: 1,
  single_click: "drop",
  double_click: "launch",
  live2d_root: "",
  disabled: [],
};

let settingsCache: FloatSettings = { ...DEFAULT_SETTINGS };
let settingsWatched = false;

export function currentSettings(): FloatSettings {
  return settingsCache;
}

export async function refreshSettings(): Promise<void> {
  try {
    const s = await invoke<FloatSettings>("floaty_get_settings");
    settingsCache = { ...DEFAULT_SETTINGS, ...s };
  } catch {
    /* keep last */
  }
}

/** Load once + stay live via backend change events. Safe to call per window. */
export function watchSettings(): void {
  if (settingsWatched) return;
  settingsWatched = true;
  void refreshSettings();
  listen<FloatSettings>("floaty-settings-changed", (e) => {
    settingsCache = { ...DEFAULT_SETTINGS, ...e.payload };
  }).catch(() => undefined);
}

// ---------- resize handle (notes + clocks) ----------

/** Bottom-right corner grip that resizes the window and persists its size. */
export function addResizeHandle(
  wrap: HTMLElement,
  rec: WidgetRecord,
  minW: number,
  minH: number,
): void {
  wrap.style.position = "relative";
  const grip = document.createElement("div");
  grip.className = "resize-handle";
  grip.title = "Resize";
  wrap.append(grip);
  grip.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    void (async () => {
      let scale = 1;
      try {
        const s = await appWin.scaleFactor();
        if (s > 0) scale = s;
      } catch {
        /* keep */
      }
      let startW = 0;
      let startH = 0;
      try {
        const sz = await appWin.innerSize();
        startW = sz.width / scale;
        startH = sz.height / scale;
      } catch {
        return;
      }
      const startSX = e.screenX;
      const startSY = e.screenY;
      const onMove = (ev: PointerEvent) => {
        const w = Math.max(minW, startW + (ev.screenX - startSX));
        const h = Math.max(minH, startH + (ev.screenY - startSY));
        void appWin
          .setSize(new PhysicalSize(Math.round(w * scale), Math.round(h * scale)))
          .catch(() => undefined);
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("pointercancel", onUp);
        void (async () => {
          try {
            const sz = await appWin.innerSize();
            rec.data["w"] = Math.round(sz.width / scale);
            rec.data["h"] = Math.round(sz.height / scale);
            await saveRecord(rec);
          } catch {
            /* ignore */
          }
        })();
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onUp);
    })();
  });
}

// ---------- pin-on-top context menu ----------

/** Right-click menu with a per-widget pin-on-top toggle. Applies instantly
 * and persists in the record; everything defaults to the desktop layer. */
export function addPinMenu(wrap: HTMLElement, getRec: () => WidgetRecord | undefined): void {
  const cur = getRec();
  if (cur && cur.data["on_top"] === true) {
    void appWin.setAlwaysOnTop(true).catch(() => undefined);
  }
  wrap.addEventListener("contextmenu", (e) => {
    const t = e.target as HTMLElement | null;
    if (t instanceof Element && t.closest("textarea, input")) return; // keep native edit menus
    e.preventDefault();
    e.stopPropagation();
    document.querySelectorAll(".pin-menu").forEach((m) => m.remove());
    const rec = getRec();
    if (!rec) return;
    const menu = document.createElement("div");
    menu.className = "pin-menu";
    const label = document.createElement("label");
    label.className = "pin-row";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = rec.data["on_top"] === true;
    box.addEventListener("click", (ev) => ev.stopPropagation());
    box.addEventListener("change", () => {
      rec.data["on_top"] = box.checked;
      void appWin.setAlwaysOnTop(box.checked).catch(() => undefined);
      void saveRecord(rec).catch(() => undefined);
      menu.remove();
    });
    const txt = document.createElement("span");
    txt.textContent = "Pin on top";
    label.append(box, txt);
    menu.append(label);
    menu.style.left = `${Math.max(4, Math.min(e.clientX, window.innerWidth - 140))}px`;
    menu.style.top = `${Math.max(4, Math.min(e.clientY, window.innerHeight - 48))}px`;
    document.body.append(menu);
    window.setTimeout(() => {
      const dismiss = (ev: PointerEvent) => {
        if (!menu.contains(ev.target as Node)) {
          menu.remove();
          window.removeEventListener("pointerdown", dismiss, true);
        }
      };
      window.addEventListener("pointerdown", dismiss, true);
    }, 0);
  });
}

// ---------- plugin enable/disable ----------

interface PluginStateMsg {
  id: string;
  enabled: boolean;
}

/** Close this window itself when its plugin is disabled (no main-thread
 * round-trip needed, so it works even when backend window ops stall). */
export function watchPluginEnabled(kind: string): void {
  listen<PluginStateMsg[]>("floaty-plugins-changed", (e) => {
    const p = e.payload.find((p) => p.id === kind);
    if (p && !p.enabled) {
      void appWin.close().catch(() => undefined);
    }
  }).catch(() => undefined);
}

let live2dCorePromise: Promise<void> | undefined;
/** Loads both Cubism 2 and Cubism 4 core runtimes. Must resolve BEFORE
 * pixi-live2d-display is imported — it throws at module evaluation if
 * either window.Live2D or window.Live2DCubismCore is absent. */
export function ensureLive2DCore(): Promise<void> {
  const win = window as unknown as { Live2D?: unknown; Live2DCubismCore?: unknown };
  if (win.Live2D && win.Live2DCubismCore) return Promise.resolve();
  if (!live2dCorePromise) {
    const loadScript = (src: string) =>
      new Promise<void>((resolve, reject) => {
        const s = document.createElement("script");
        s.src = src;
        s.onload = () => resolve();
        s.onerror = () => reject(new Error(`Failed to load ${src}`));
        document.head.append(s);
      });
    live2dCorePromise = Promise.all([
      win.Live2D ? Promise.resolve() : loadScript("/live2d.min.js"),
      win.Live2DCubismCore ? Promise.resolve() : loadScript("/live2dcubismcore.min.js"),
    ])
      .then(() => undefined)
      .catch((err) => {
        live2dCorePromise = undefined;
        throw err;
      });
  }
  return live2dCorePromise;
}
