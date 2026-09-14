import { invoke } from "@tauri-apps/api/core";
import { desktopItemFor, pathOf } from "./pluginManifest";
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
if (appWin.label.startsWith("widget-")) {
  void appWin.setSkipTaskbar(true).catch(() => undefined);
}

export const DESKTOP_LAYER = "desktop-overlay";
export const TOP_LAYER = "top-overlay";

export function isOverlayMode(): boolean {
  return (
    appWin.label === DESKTOP_LAYER ||
    appWin.label === TOP_LAYER ||
    window.location.hash.startsWith("#/overlay")
  );
}

/**
 * Whether this page is the always-on-top layer, the one that draws the widgets
 * pinned above other windows. Both layers are full-screen overlays running the
 * same mount code; they differ in which records they draw.
 */
export function isTopLayer(): boolean {
  return appWin.label === TOP_LAYER;
}

/** Whether a record belongs in this layer rather than the desktop one. */
export function isPinned(rec: WidgetRecord): boolean {
  return rec.data["on_top"] === true;
}

export interface OverlaySlot {
  id: string;
  kind: string;
  x: number;
  y: number;
  w: number;
  h: number;
  element: HTMLElement;
}

export const overlaySlots = new Map<string, OverlaySlot>();

export function registerOverlaySlot(slot: OverlaySlot): void {
  overlaySlots.set(slot.id, slot);
  scheduleHitRectsUpdate();
}

export function unregisterOverlaySlot(id: string): void {
  overlaySlots.delete(id);
  scheduleHitRectsUpdate();
}

let hitRectsTimer: number | undefined;
let lastHitRectsKey = "";
export function scheduleHitRectsUpdate(): void {
  if (!isOverlayMode()) return;
  if (hitRectsTimer !== undefined) return;
  hitRectsTimer = window.setTimeout(() => {
    hitRectsTimer = undefined;
    syncHitRectsToRust();
  }, 50);
}

export function syncHitRectsToRust(): void {
  if (!isOverlayMode()) return;
  const dpr = window.devicePixelRatio || 1;
  const rects: Array<{ id: string; x: number; y: number; w: number; h: number }> = [];

  for (const slot of overlaySlots.values()) {
    const el = slot.element;
    const r = el.getBoundingClientRect();
    if (r.width > 0 && r.height > 0) {
      const pad = 8;
      rects.push({
        id: slot.id,
        x: Math.max(0, Math.floor((r.left - pad) * dpr)),
        y: Math.max(0, Math.floor((r.top - pad) * dpr)),
        w: Math.ceil((r.width + pad * 2) * dpr),
        h: Math.ceil((r.height + pad * 2) * dpr),
      });
    }
  }

  const popups = document.querySelectorAll(".pin-menu, .model-menu, .live2d-palette-modal, .live2d-palette, .fname-edit");
  popups.forEach((p, idx) => {
    const r = p.getBoundingClientRect();
    if (r.width > 0 && r.height > 0) {
      const pad = 4;
      rects.push({
        id: `popup-${idx}`,
        x: Math.max(0, Math.floor((r.left - pad) * dpr)),
        y: Math.max(0, Math.floor((r.top - pad) * dpr)),
        w: Math.ceil((r.width + pad * 2) * dpr),
        h: Math.ceil((r.height + pad * 2) * dpr),
      });
    }
  });

  const key = rects.map((r) => `${r.id}:${r.x},${r.y},${r.w},${r.h}`).join(";");
  if (key === lastHitRectsKey) return;
  lastHitRectsKey = key;

  // The label says *which* overlay these rects are the click-through region of:
  // there are two of them (the desktop layer and the always-on-top layer), and a
  // region applied to the wrong window blocks the screen or the widgets.
  invoke("floaty_update_hit_rects", { label: appWin.label, rects }).catch(() => undefined);
}

let overlayDragging = false;
export function notifyDragging(dragging: boolean): void {
  overlayDragging = dragging;
  if (isOverlayMode()) {
    if (!dragging) {
      syncHitRectsToRust();
    }
    // per overlay layer: the drag only clears the region of the window being
    // dragged in, or the other layer loses its clicks for the duration
    invoke("floaty_set_overlay_dragging", { label: appWin.label, dragging }).catch(
      () => undefined,
    );
    if (!dragging) {
      scheduleHitRectsUpdate();
    }
  }
  if (!dragging) {
    // the end of a drag is what takes the merge preview off the target tile
    window.dispatchEvent(new CustomEvent("floaty-drag-end"));
  }
}

/**
 * Where a dragged tile is right now, so the overlay can show what dropping it
 * there would do.
 *
 * Only the overlay page keeps several widgets in one document, so it is the one
 * page that can preview a merge; in per-window mode nothing listens and this
 * costs a CustomEvent. `x`/`y` are the tile's own desktop coordinates, the same
 * ones `floaty_dropped` is given on release.
 */
export function notifyDragMove(id: string, x: number, y: number): void {
  window.dispatchEvent(new CustomEvent("floaty-drag-move", { detail: { id, x, y } }));
}

export function setWidgetPos(id: string, x: number, y: number, scale = 1): void {
  if (isOverlayMode()) {
    const slot = overlaySlots.get(id);
    if (slot) {
      slot.x = x;
      slot.y = y;
      slot.element.style.transform = `translate3d(${x}px, ${y}px, 0)`;
      if (!overlayDragging) {
        scheduleHitRectsUpdate();
      }
    }
  } else {
    void appWin.setPosition(new PhysicalPosition(Math.round(x * scale), Math.round(y * scale))).catch(() => undefined);
  }
}

export function setWidgetSize(id: string, w: number, h: number, scale = 1): void {
  if (isOverlayMode()) {
    const slot = overlaySlots.get(id);
    if (slot) {
      slot.w = w;
      slot.h = h;
      slot.element.style.width = `${w}px`;
      slot.element.style.height = `${h}px`;
      scheduleHitRectsUpdate();
    }
  } else {
    void appWin.setSize(new PhysicalSize(Math.round(w * scale), Math.round(h * scale))).catch(() => undefined);
  }
}

async function scaleFactor(): Promise<number> {
  try {
    const s = await appWin.scaleFactor();
    return s > 0 ? s : 1;
  } catch {
    return 1;
  }
}

/** Window position in logical px. If id is passed and in overlay mode, gets slot pos. */
export async function logicalPos(id?: string): Promise<{ x: number; y: number }> {
  if (isOverlayMode()) {
    if (id) {
      const slot = overlaySlots.get(id);
      if (slot) return { x: slot.x, y: slot.y };
    }
    return { x: 0, y: 0 };
  }
  const s = await scaleFactor();
  const p = await appWin.outerPosition();
  return { x: p.x / s, y: p.y / s };
}

export async function setLogicalPos(x: number, y: number, id?: string): Promise<void> {
  if (isOverlayMode() && id) {
    setWidgetPos(id, x, y);
    return;
  }
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
  if (isOverlayMode()) {
    return {
      x: 0,
      y: 0,
      w: window.innerWidth || 1920,
      h: window.innerHeight || 1080,
    };
  }
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
  // One record per widget, never the whole store: `floaty_list` carries every
  // icon inlined (~8.5MB) and this runs once per widget on a page load.
  return await invoke<WidgetRecord | null>("floaty_get_record", { id }).then((r) => r ?? undefined);
}

export async function saveRecord(rec: WidgetRecord): Promise<void> {
  await invoke("floaty_save", { record: rec });
}

/** Clamp helper used by the float-parameter plumbing (keeps NaN out of CSS). */
function clampNum(v: number, min: number, max: number): number {
  return Number.isFinite(v) ? Math.min(max, Math.max(min, v)) : min;
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
const trackedPositions = new Set<string>();

export function trackPosition(rec: WidgetRecord): void {
  if (trackedPositions.has(rec.id)) return; // one watcher per widget
  trackedPositions.add(rec.id);
  const snap = async () => {
    try {
      if (isOverlayMode()) {
        const slot = overlaySlots.get(rec.id);
        if (!slot) return;
        const x = Math.round(slot.x);
        const y = Math.round(slot.y);
        if (x !== rec.x || y !== rec.y) {
          rec.x = x;
          rec.y = y;
          await saveRecord(rec);
        }
        return;
      }
      const p = await logicalPos(rec.id);
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
  if (!isOverlayMode()) {
    window.setInterval(() => void snap(), 2000);
  }
  window.addEventListener("beforeunload", () => void snap());
  // persist the moment a drag ends or the window hides instead of waiting
  // for the next poll — quits otherwise lose the last move
  window.addEventListener("pointerup", () => window.setTimeout(() => void snap(), 60));
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) void snap();
  });
}

/**
 * Anti-collision helper: ensures a widget placed at (x, y) does not overlap
 * any existing widget in the overlay, nudging it to the nearest collision-free position.
 */
export function preventOverlap(
  id: string,
  x: number,
  y: number,
  w: number,
  h: number,
  mon?: MonitorArea,
  gap = 8,
): { x: number; y: number } {
  if (!isOverlayMode()) {
    return { x, y };
  }

  const screenX = mon ? mon.x : 0;
  const screenY = mon ? mon.y : 0;
  const screenW = mon && mon.w > 0 ? mon.w : (window.innerWidth || 1920);
  const screenH = mon && mon.h > 0 ? mon.h : (window.innerHeight || 1080);
  const minX = screenX + 16;
  const minY = screenY + 16;
  const maxX = Math.max(minX, screenX + screenW - w - 16);
  const maxY = Math.max(minY, screenY + screenH - h - 16);

  const clampX = (val: number) => Math.max(minX, Math.min(maxX, val));
  const clampY = (val: number) => Math.max(minY, Math.min(maxY, val));

  const curX = clampX(x);
  const curY = clampY(y);

  // Collect other slots
  const others: OverlaySlot[] = [];
  for (const slot of overlaySlots.values()) {
    if (slot.id !== id) {
      others.push(slot);
    }
  }

  const collidesWithAny = (cx: number, cy: number): boolean => {
    for (const o of others) {
      const ox = Math.min(cx + w, o.x + o.w) - Math.max(cx, o.x);
      const oy = Math.min(cy + h, o.y + o.h) - Math.max(cy, o.y);
      if (ox > 12 && oy > 12) {
        return true;
      }
    }
    return false;
  };

  if (!collidesWithAny(curX, curY)) {
    return { x: curX, y: curY };
  }

  // Collision detected: find candidate positions adjacent to the colliding slots
  const colliding = others.filter((o) => {
    const ox = Math.min(curX + w, o.x + o.w) - Math.max(curX, o.x);
    const oy = Math.min(curY + h, o.y + o.h) - Math.max(curY, o.y);
    return ox > 12 && oy > 12;
  });

  type Candidate = { x: number; y: number; dist: number };
  const candidates: Candidate[] = [];

  for (const o of colliding) {
    const shifts = [
      { x: clampX(o.x + o.w + gap + 4), y: clampY(curY) }, // right
      { x: clampX(o.x - w - gap - 4), y: clampY(curY) },   // left
      { x: clampX(curX), y: clampY(o.y + o.h + gap + 4) }, // below
      { x: clampX(curX), y: clampY(o.y - h - gap - 4) },   // above
      { x: clampX(o.x + o.w + gap + 4), y: clampY(o.y) },
      { x: clampX(o.x - w - gap - 4), y: clampY(o.y) },
    ];
    for (const s of shifts) {
      if (!collidesWithAny(s.x, s.y)) {
        candidates.push({
          x: s.x,
          y: s.y,
          dist: Math.hypot(s.x - curX, s.y - curY),
        });
      }
    }
  }

  if (candidates.length > 0) {
    candidates.sort((a, b) => a.dist - b.dist);
    return { x: candidates[0].x, y: candidates[0].y };
  }

  // If immediate directional shifts failed, search nearby desktop grid positions
  const CELL_W = 100;
  const CELL_H = 116;
  const baseCol = Math.round((curX - minX) / CELL_W);
  const baseRow = Math.round((curY - minY) / CELL_H);

  for (let r = 1; r <= 15; r++) {
    for (let dc = -r; dc <= r; dc++) {
      for (let dr = -r; dr <= r; dr++) {
        if (Math.max(Math.abs(dc), Math.abs(dr)) === r) {
          const gx = clampX(minX + (baseCol + dc) * CELL_W);
          const gy = clampY(minY + (baseRow + dr) * CELL_H);
          if (!collidesWithAny(gx, gy)) {
            return { x: gx, y: gy };
          }
        }
      }
    }
  }

  return { x: curX, y: curY };
}

export interface OverlayDragOptions {
  /** Pointer travel (px) before a press counts as a drag rather than a click. */
  threshold?: number;
}

/**
 * Make an element drag its widget around. Title bars use this, and so do
 * widgets with no bar (their whole surface is the handle).
 *
 * A press only becomes a drag after `threshold` px of travel, so widgets that
 * also react to plain clicks (visualizer modes, sysmon graph) keep working:
 * a press that really moved swallows the click that follows it.
 */
export function enableOverlayDrag(
  el: HTMLElement,
  id: string | undefined,
  opts: OverlayDragOptions = {},
): void {
  const threshold = opts.threshold ?? 4;

  // Swallow the click that follows a real drag: it must not reach the widget's
  // own click action. Installed at the window in the capture phase, because a
  // listener on the widget itself would run *after* the widget's own handler.
  const swallowClick = (e: Event): void => {
    e.stopPropagation();
    e.preventDefault();
  };
  const stopSwallowing = (): void => window.removeEventListener("click", swallowClick, true);

  // controls inside the draggable surface keep their own behaviour
  const isControl = (target: EventTarget | null): boolean => {
    const t = target as HTMLElement | null;
    return !!t?.closest?.("button, input, textarea, select, .resize-handle, .pin-menu, .model-menu");
  };

  el.addEventListener(
    "pointerdown",
    (e) => {
      if (e.button !== 0) return;
      if (isControl(e.target)) return;
      stopSwallowing();
      if (!isOverlayMode() || !id) {
        void appWin.startDragging().catch(() => undefined);
        return;
      }
      const slot = overlaySlots.get(id);
      if (!slot) return;
      e.stopPropagation();
      const startX = slot.x;
      const startY = slot.y;
      const startSX = e.clientX;
      const startSY = e.clientY;
      let active = false;

      const onMove = (ev: PointerEvent) => {
        const dx = ev.clientX - startSX;
        const dy = ev.clientY - startSY;
        if (!active) {
          if (Math.abs(dx) < threshold && Math.abs(dy) < threshold) return;
          active = true;
          window.addEventListener("click", swallowClick, true);
          notifyDragging(true);
        }
        setWidgetPos(id, startX + dx, startY + dy);
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("pointercancel", onUp);
        if (active) {
          notifyDragging(false);
          // the click lands in the same gesture; drop the guard right after
          window.setTimeout(stopSwallowing, 350);
        }
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onUp);
    },
    // capture, so a widget's own pointerdown handler (which stops propagation)
    // cannot swallow the drag
    true,
  );
}

/**
 * Title row for the widgets that have one — the note and the clock.
 *
 * It reads like the system monitor's header: a small letterspaced caption and a
 * close button that stays quiet until the widget is under the cursor. No dots,
 * no coloured strip: a panel floats on the wallpaper and the chrome stays part
 * of the panel (`--panel-*` in style.css).
 */
export function makeBar(title: string, onClose: () => void, id?: string): HTMLElement {
  const bar = document.createElement("div");
  bar.className = "bar";
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
  bar.append(label, close);
  enableOverlayDrag(bar, id);
  return bar;
}

/**
 * Ask the user before dropping a floatie. Removing a file/folder floatie only
 * takes it off the desktop (nothing is deleted on disk), but the widget, its
 * pinned spot and its folder membership are gone, so it is worth a question.
 * `confirm_remove` in settings turns this off.
 */
export async function confirmRemoveDialog(what: string, kind: string): Promise<boolean> {
  const item = desktopItemFor(kind);
  const onDisk = item !== undefined;
  const noun = item?.noun ?? `${kind} floatie`;
  const body = onDisk
    ? `Delete this ${noun}?\n\n${what}\n\nIf it lives on your desktop it goes to the Recycle Bin (recoverable); anything outside the desktop only loses its floatie.`
    : `Remove this ${noun} from the desktop?\n\n${what}`;
  try {
    const { confirm } = await import("@tauri-apps/plugin-dialog");
    return await confirm(body, {
      title: "floaty",
      kind: "warning",
      okLabel: "remove",
      cancelLabel: "keep",
    });
  } catch {
    // dialog plugin unavailable (or a window that cannot raise a modal):
    // fall back to the webview's own confirm so a removal is never silent
    return window.confirm(body);
  }
}

/** Short label for a record: its name plus the path it points at — through
 *  `pathOf`, so a widget that happens to store its own `target` (the countdown's
 *  date) is not described as if that were a file. */
export function describeForConfirm(rec: WidgetRecord): string {
  const name =
    typeof rec.data["name"] === "string" && (rec.data["name"] as string)
      ? (rec.data["name"] as string)
      : rec.id;
  const target = pathOf(rec);
  return target ? `${name}\n${target}` : name;
}

/**
 * Remove a floatie. Files, folders and shortcuts that live on the desktop are
 * deleted for real — the backend sends them to the Recycle Bin so the widget
 * and the disk stay in step — while widgets that are not desktop items only
 * stop floating.
 */
export async function removeSelf(rec: WidgetRecord): Promise<void> {
  if (currentSettings().confirm_remove && !(await confirmRemoveDialog(describeForConfirm(rec), rec.kind))) {
    return; // user kept it
  }
  try {
    // Desktop items are deleted on disk (the backend sends them to the Recycle
    // Bin); widgets that are not desktop items — and desktop items whose record
    // has no path yet, such as a folder just made by grouping — only stop
    // floating. Which is which comes from the plugin manifest, not from a list
    // kept here.
    if (hasDesktopPath(rec)) {
      await invoke<string>("floaty_delete", { id: rec.id });
    } else {
      await invoke("floaty_remove", { id: rec.id });
    }
  } catch (err) {
    // A refusal from the backend must not take the window down: in the overlay
    // that window *is* the desktop, so one pathless folder used to close every
    // widget at once (the log said nothing, the desktop just went empty).
    // Closing is only a way out for a window that holds this widget and nothing
    // else: in the overlay the window *is* the desktop, and in the settings
    // window it is the settings window, so neither may be closed over one
    // refusal.
    void invoke("floaty_log", {
      msg: `[self] removing ${rec.id} (${rec.kind}) failed: ${String(err)}`,
    }).catch(() => undefined);
    if (!appWin.label.startsWith("widget-")) return;
    await appWin.close().catch(() => undefined);
  }
}

/**
 * Tell the backend this page is alive: after a sleep/standby the app can end up
 * with a wedged renderer, and the only way to tell is that the page stops
 * reporting in (see `recover_windows` in the backend).
 */
/**
 * Send one beat immediately. A hidden page has its timers throttled, so the
 * interval in `startHeartbeat` can be a minute apart right when the backend most
 * needs to know (this is called from `visibilitychange`, and events are not
 * throttled).
 */
export function beatNow(label: string): void {
  void invoke("floaty_heartbeat", { label, visibility: document.visibilityState }).catch(
    () => undefined,
  );
}

export function startHeartbeat(label: string): void {
  // The first beat is sent straight away and is not a timer, so it is not
  // throttled: it is the one signal the backend can trust after a standby, and
  // it says whether this page believes it is visible (a page stuck believing it
  // is hidden has its timers throttled and its widgets look frozen).
  const beat = () =>
    void invoke("floaty_heartbeat", { label, visibility: document.visibilityState }).catch(
      () => undefined,
    );
  beat();
  window.setInterval(beat, 5000);
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
  files_root: string;
  disabled: string[];
  stay_on_desktop: boolean;
  animated_ratio: number;
  animation_mode: string;
  /** base float travel in px (icons bob this far) */
  float_amplitude: number;
  /** seconds per float cycle (lower = floating more often) */
  float_period: number;
  /** how far apart neighbouring floaties bob (percent of the stagger) */
  float_spread: number;
  /** ask before removing a file, folder or widget */
  confirm_remove: boolean;
  /** visualizer sensitivity multiplier */
  viz_gain: number;
  /** visualizer redraw rate while audio plays */
  viz_fps: number;
  /** system monitor sample period in ms */
  sysmon_interval: number;
}

export const DEFAULT_SETTINGS: FloatSettings = {
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
  float_amplitude: 4.5,
  float_period: 10,
  float_spread: 100,
  confirm_remove: true,
  viz_gain: 1,
  viz_fps: 30,
  sysmon_interval: 1000,
};

/**
 * Configures the floating animation classes and styles for app icons and folders
 * based on currentSettings().floatiness, .animated_ratio, and .animation_mode.
 */
export function applyFloatieAnimation(wrap: HTMLElement, id: string): void {
  const s = currentSettings();
  const f = typeof s.floatiness === "number" ? s.floatiness : 1;
  wrap.style.setProperty("--float", String(f));

  const ratio = typeof s.animated_ratio === "number" ? s.animated_ratio : 100;
  const mode = s.animation_mode || "wave";

  // Live float parameters. `floatiness` stays the coarse "how floaty" knob and
  // scales the base travel from settings; period and spread are read straight
  // from the sliders (settings-changed re-runs this, so edits apply instantly).
  const ampBase = typeof s.float_amplitude === "number" ? s.float_amplitude : 4.5;
  const period = typeof s.float_period === "number" ? s.float_period : 10;
  const spread = typeof s.float_spread === "number" ? s.float_spread : 100;
  const amp = clampNum(ampBase * f, 0, 24);
  wrap.style.setProperty("--bob-amp", `${amp}px`);
  wrap.style.setProperty("--bob-cycle", `${clampNum(period, 2, 60)}s`);
  wrap.style.setProperty("--bob-spread", String(clampNum(spread / 100, 0, 2)));

  // The bob's step counts live in style.css as literal `steps(n)`: Chromium
  // silently drops `steps(var(--n))`, and a generated staircase with a stop per
  // step measured 5x the GPU of the compact form while looking the same.

  wrap.classList.remove(
    "anim-wave",
    "anim-sync",
    "anim-gentle",
    "anim-static",
    "phase-0",
    "phase-1",
    "phase-2",
    "phase-3",
    "phase-4",
    "phase-5",
  );

  if (mode === "static" || ratio <= 0 || f <= 0) {
    wrap.classList.add("anim-static");
    return;
  }

  // Deterministic numeric index from widget ID (e.g. app-17 -> 17)
  const numDigits = parseInt(id.replace(/\D/g, ""), 10);
  const num = Number.isFinite(numDigits)
    ? numDigits
    : Math.abs(Array.from(id).reduce((acc, c) => (acc * 31 + c.charCodeAt(0)) | 0, 0));

  // Determine if this specific floatie is included in the motion ratio
  const isAnimated = ((num * 37) % 100) < ratio;
  if (!isAnimated) {
    wrap.classList.add("anim-static");
    return;
  }

  wrap.classList.add(`anim-${mode}`);
  if (mode === "wave") {
    wrap.classList.add(`phase-${num % 6}`);
  }
}

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

// ---------- icon storage ----------

/** Pixel size of a stored icon, read straight out of the PNG header. */
function iconSize(url: string): { w: number; h: number } | undefined {
  const b64 = url.startsWith("data:") ? url.slice(url.indexOf(",") + 1) : url;
  try {
    const head = atob(b64.slice(0, 40));
    if (head.length < 24 || head.charCodeAt(0) !== 0x89) return undefined;
    const w =
      ((head.charCodeAt(16) << 24) |
        (head.charCodeAt(17) << 16) |
        (head.charCodeAt(18) << 8) |
        head.charCodeAt(19)) >>>
      0;
    const h =
      ((head.charCodeAt(20) << 24) |
        (head.charCodeAt(21) << 16) |
        (head.charCodeAt(22) << 8) |
        head.charCodeAt(23)) >>>
      0;
    return { w, h };
  } catch {
    return undefined;
  }
}

/** Nothing renderable stored yet: empty, "none", or a data url we cannot read. */
export function iconIsMissing(url: string | undefined): boolean {
  if (!url || url === "none") return true;
  return iconSize(url) === undefined;
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
    notifyDragging(true);
    void (async () => {
      let scale = 1;
      try {
        const s = await appWin.scaleFactor();
        if (s > 0) scale = s;
      } catch {
        /* keep */
      }
      let startW = wrap.offsetWidth;
      let startH = wrap.offsetHeight;
      if (!isOverlayMode()) {
        try {
          const sz = await appWin.innerSize();
          startW = sz.width / scale;
          startH = sz.height / scale;
        } catch {
          return;
        }
      }
      const isOverlay = isOverlayMode();
      const startSX = isOverlay ? e.clientX : e.screenX;
      const startSY = isOverlay ? e.clientY : e.screenY;
      const onMove = (ev: PointerEvent) => {
        const curSX = isOverlay ? ev.clientX : ev.screenX;
        const curSY = isOverlay ? ev.clientY : ev.screenY;
        const w = Math.max(minW, startW + (curSX - startSX));
        const h = Math.max(minH, startH + (curSY - startSY));
        setWidgetSize(rec.id, w, h, scale);
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("pointercancel", onUp);
        notifyDragging(false);
        rec.data["w"] = Math.round(wrap.offsetWidth);
        rec.data["h"] = Math.round(wrap.offsetHeight);
        void saveRecord(rec);
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onUp);
    })();
  });
}

// ---------- pin-on-top context menu ----------

/**
 * What a plugin can add to its own widget's right-click menu.
 *
 * The standard rows (file operations, pin on top, removal) are floaty's
 * business; a plugin's own rows are the plugin's, and it is handed this in
 * `addPinMenu`'s `rows` option.
 */
export interface PinMenuApi {
  /** A row that runs `fn` and closes the menu. */
  run: (label: string, fn: () => unknown) => void;
  /** A bare row: the plugin attaches its own click handler, and decides whether
   *  the menu closes. */
  row: (label: string) => HTMLElement;
  /** Rule between groups of rows. */
  divider: () => void;
  /** Short line at the top of the menu, e.g. "model changed". */
  note: (msg: string) => void;
  /** Remove the menu (and its hit rect). */
  close: () => void;
  /**
   * Keep the menu mounted but draw something else in it.
   *
   * This is how a row that needs a list replaces the commands instead of
   * stacking a second popup on top of them (Rename… swaps in an input, the
   * live2d model picker a scrolling list). `fill` gets the menu to empty and
   * `done` to remove it when the choice is made.
   */
  swap: (fill: (body: HTMLElement, done: () => void) => void) => void;
}

export interface PinMenuOptions {
  /** Rows for this widget, drawn above the standard ones. */
  rows?: (api: PinMenuApi) => void;
}

/**
 * Put this window into the desktop layer.
 *
 * Only meaningful for a window that holds a single widget: an overlay window's
 * z-order is the backend's business — the desktop layer, or the always-on-top
 * layer a pinned widget is drawn in (`floaty_set_on_top`).
 */
export function enforceDesktopLayer(): void {
  if (isOverlayMode()) return;
  void appWin.setAlwaysOnTop(false).catch(() => undefined);
}

/** Right-click menu with a per-widget pin-on-top toggle. Applies instantly
 * and persists in the record; everything defaults to the desktop layer.
 * `options.rows` lets a plugin add its own rows to its widget's menu. */
export function addPinMenu(
  wrap: HTMLElement,
  getRec: () => WidgetRecord | undefined,
  options?: PinMenuOptions,
): void {
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

    const closeMenu = (): void => {
      menu.remove();
      scheduleHitRectsUpdate();
    };
    const note = (msg: string): void => {
      const line = document.createElement("div");
      line.className = "pin-note";
      line.textContent = msg;
      menu.prepend(line);
      window.setTimeout(() => line.remove(), 2200);
    };
    // Row factory: rows are appended immediately, but only `run` rows close the
    // menu — an inline rename needs the menu to stay mounted while it swaps in
    // its input, so it must not go through `run`.
    const addRow = (label: string, danger = false): HTMLElement => {
      const row = document.createElement("div");
      row.className = danger ? "pin-row pin-btn danger" : "pin-row pin-btn";
      row.textContent = label;
      menu.append(row);
      return row;
    };
    const run = (label: string, fn: () => Promise<unknown>, danger = false): void => {
      const row = addRow(label, danger);
      row.addEventListener("click", (ev) => {
        ev.stopPropagation();
        closeMenu();
        void fn().catch((err: unknown) => {
          void invoke("floaty_log", { msg: `[menu] ${label} failed: ${String(err)}` }).catch(() => undefined);
        });
      });
    };
    const divider = (): void => {
      const hr = document.createElement("div");
      hr.className = "pin-sep";
      menu.append(hr);
    };

    // The plugin's own rows go above the standard ones: they are what the
    // widget is about, and this is the only menu it has.
    options?.rows?.({
      run: (label, fn) => {
        run(label, () => Promise.resolve(fn()));
      },
      row: (label) => addRow(label),
      divider,
      note,
      close: closeMenu,
      swap: (fill) => {
        menu.textContent = "";
        fill(menu, closeMenu);
        scheduleHitRectsUpdate();
      },
    });

    // Files, folders and shortcut floaties get the desktop's own operations;
    // which kinds those are is the manifest's business, not this file's.
    const path = pathOf(rec);
    const isFileish = path !== "" && hasDesktopPath(rec);
    if (isFileish) {
      run("Open", () => invoke("floaty_launch", { id: rec.id }));
      run("Open with…", () => invoke("floaty_open_with", { id: rec.id }));
      run("Show in File Explorer", () => invoke("floaty_reveal", { id: rec.id }));
      // Inline rename: click keeps the menu mounted and startRename() replaces
      // its contents with the input (it removes the menu itself when done).
      const renameRow = addRow("Rename…");
      renameRow.addEventListener("click", (ev) => {
        ev.stopPropagation();
        void startRename(menu, rec, note).catch(() => {
          closeMenu();
        });
      });
      run("Copy path", async () => {
        await navigator.clipboard.writeText(path);
        note("path copied");
      });
      run("Properties", () => invoke("floaty_properties", { id: rec.id }));
      divider();
    }

    const label = document.createElement("label");
    label.className = "pin-row";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = rec.data["on_top"] === true;
    box.addEventListener("click", (ev) => ev.stopPropagation());
    box.addEventListener("change", () => {
      const onTop = box.checked;
      rec.data["on_top"] = onTop;
      // The backend owns this one: a pinned widget is drawn in the other overlay
      // layer, so it has to be re-homed, which a flag set from here cannot do.
      void invoke("floaty_set_on_top", { id: rec.id, onTop }).catch((err: unknown) => {
        void invoke("floaty_log", {
          msg: `[menu] pin on top ${rec.id} failed: ${String(err)}`,
        }).catch(() => undefined);
      });
      void saveRecord(rec).catch(() => undefined);
      closeMenu();
    });
    const txt = document.createElement("span");
    txt.textContent = "Pin on top";
    label.append(box, txt);
    menu.append(label);

    if (isFileish) {
      divider();
      run("Delete (Recycle Bin)", () => removeSelf(rec), true);
    }

    const menuW = 216;
    menu.style.left = `${Math.max(4, Math.min(e.clientX, window.innerWidth - menuW))}px`;
    menu.style.top = `${Math.max(4, Math.min(e.clientY, window.innerHeight - 48))}px`;
    document.body.append(menu);
    scheduleHitRectsUpdate();
    window.setTimeout(() => {
      const dismiss = (ev: PointerEvent) => {
        if (!menu.contains(ev.target as Node)) {
          menu.remove();
          scheduleHitRectsUpdate();
          window.removeEventListener("pointerdown", dismiss, true);
        }
      };
      window.addEventListener("pointerdown", dismiss, true);
    }, 0);
  });
}

/**
 * True when this record points at something on disk, so removing it should
 * delete for real (through the Recycle Bin) instead of only un-floating it.
 *
 * `pathOf` answers both halves: it is "" for a kind the manifest does not call a
 * desktop item, and "" for a record of such a kind that carries no path. Asking
 * about the *kind* alone (which is what this used to do) sent a folder made
 * in-app by grouping — a folder with no path until it is filled — to
 * `floaty_delete`, and the backend's "this folder has no path" is what the
 * removal path below then treated as a failure.
 */
function hasDesktopPath(rec: WidgetRecord): boolean {
  return pathOf(rec) !== "";
}

/** Inline rename, replacing the menu with an input pre-filled with the name. */
function startRename(
  menu: HTMLElement,
  rec: WidgetRecord,
  note: (msg: string) => void,
): Promise<void> {
  return new Promise<void>((resolve) => {
    const current = pathOf(rec);
    const base = current.split(/[\\/]/).pop() ?? "";
    menu.textContent = "";
    const input = document.createElement("input");
    input.className = "pin-input";
    input.value = base;
    menu.append(input);
    const hint = document.createElement("div");
    hint.className = "pin-note";
    hint.textContent = "enter to rename · esc to cancel";
    menu.append(hint);
    input.addEventListener("pointerdown", (ev) => ev.stopPropagation());
    input.focus();
    const dot = base.lastIndexOf(".");
    if (dot > 0) input.setSelectionRange(0, dot);
    else input.select();

    let done = false; // the input is gone and the promise has resolved
    let busy = false; // a rename is in flight
    const cleanup = (): void => {
      done = true;
      menu.remove();
      scheduleHitRectsUpdate();
      resolve();
    };
    const finish = (commit: boolean): void => {
      if (done || busy) return;
      const name = input.value.trim();
      if (!commit || !name || name === base) {
        cleanup();
        return;
      }
      // Keep the menu (and the typed name) up until the backend answers, so
      // "already exists here" or "name cannot end with a dot" is visible
      // instead of silently doing nothing, and the name can be fixed.
      busy = true;
      input.disabled = true;
      invoke<WidgetRecord>("floaty_rename", { id: rec.id, name })
        .then(cleanup)
        .catch((err: unknown) => {
          busy = false;
          input.disabled = false;
          input.focus();
          note(String(err));
          void invoke("floaty_log", { msg: `[menu] rename failed: ${String(err)}` }).catch(() => undefined);
        });
    };
    input.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter") finish(true);
      else if (ev.key === "Escape") finish(false);
    });
    input.addEventListener("blur", () => {
      // a blur caused by the successful cleanup must not rename twice
      if (!done) finish(true);
    });
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
    if (p && !p.enabled && !isOverlayMode()) {
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
