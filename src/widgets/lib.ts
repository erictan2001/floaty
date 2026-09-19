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

/**
 * Slots a plugin has stretched over the desktop to catch the mouse (a drawing surface, a
 * page of save slots). While a widget is reserving, the slot's position is scaffolding and
 * not the user's choice, so the position watcher must not persist it — a click on the last
 * thing that happens inside the stretch is a `pointerup`, and the watcher's snap 60ms later
 * would otherwise save (0,0) over the widget's real place.
 */
const reservingSlots = new Set<string>();

export function setWidgetPos(id: string, x: number, y: number, scale = 1, opts?: { transient?: boolean }): void {
  if (opts?.transient) reservingSlots.add(id);
  else reservingSlots.delete(id);
  if (isOverlayMode()) {
    const slot = overlaySlots.get(id);
    if (slot) {
      slot.x = x;
      slot.y = y;
      // record space in, this window's css px out: on the second screen the window's
      // own origin is 1280px to the right of the arrangement's
      const at = toWindow(x, y);
      slot.element.style.transform = `translate3d(${at.x}px, ${at.y}px, 0)`;
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
  try {
    const s = await scaleFactor();
    const p = await appWin.outerPosition();
    return { x: p.x / s, y: p.y / s };
  } catch {
    return { x: 0, y: 0 };
  }
}

export async function setLogicalPos(x: number, y: number, id?: string): Promise<void> {
  if (isOverlayMode() && id) {
    setWidgetPos(id, x, y);
    return;
  }
  try {
    const s = await scaleFactor();
    await appWin.setPosition(new PhysicalPosition(Math.round(x * s), Math.round(y * s)));
  } catch (err) {
    void invoke("floaty_log", {
      msg: `[lib] setLogicalPos failed: ${String(err)}`,
    }).catch(() => undefined);
  }
}

export interface MonitorArea {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * The screen this overlay window covers, and the desktop as a whole.
 *
 * An overlay is one window **per screen** (see `spawn_overlay_windows` in `lib.rs`), so
 * the window's own (0,0) is its screen's top-left while records — and every position a
 * widget deals with — are in the arrangement's space. `toWindow`/`toRecords` are where
 * the two meet, and with one screen they are the identity.
 */
interface OverlayArea {
  index: number;
  screen: {
    name: string;
    primary: boolean;
    scale: number;
    physical: MonitorArea;
    logical: MonitorArea;
  };
  desktop: MonitorArea;
}

let area: OverlayArea | null = null;
let areaPromise: Promise<OverlayArea | null> | null = null;

/** What is known about this window's screen right now (null before the first read). */
export function overlayArea(): OverlayArea | null {
  return area;
}

/**
 * Ask the backend which screen this window is on. Cached per window; re-read when the
 * arrangement changes.
 */
export function myArea(): Promise<OverlayArea | null> {
  areaPromise ??= invoke<OverlayArea | null>("floaty_overlay_area")
    .then((value) => {
      area = value ?? null;
      // Warm the screens here too: a drag places a floatie from the pointer's own
      // position, which is mapped through the screen it is on, and a cold cache would
      // silently fall back to the delta maths this exists to replace.
      void screens();
      return area;
    })
    .catch(() => null);
  return areaPromise;
}

function dropAreaCache(): void {
  areaPromise = null;
  screenList = null;
}

/** A screen as `floaty_screens` reports it: the same monitor in both spaces. */
interface ScreenInfo {
  key: string;
  name: string;
  physical: MonitorArea;
  logical: MonitorArea;
  scale: number;
  primary: boolean;
}

let screenList: ScreenInfo[] | null = null;

/** Every monitor, cached — the drag needs them to place a floatie on any screen. */
export async function screens(): Promise<ScreenInfo[]> {
  if (!screenList) {
    screenList = await invoke<ScreenInfo[]>("floaty_screens").catch(() => []);
  }
  return screenList ?? [];
}

/**
 * Where the pointer is *physically*, from a pointer event in this window.
 *
 * The event's css px are in this window's own space and stay in it even while the
 * pointer is captured outside the window — the ratio is anchored to this window's DPI
 * context. That is what makes this exact where accumulating deltas was not: 400 css px
 * past the right edge of a 200% screen is 800 physical px into the next screen, whose
 * own scale may be anything.
 */
function pointerPhysical(e: { clientX: number; clientY: number }): { x: number; y: number } | null {
  const s = area?.screen;
  if (!s) return null;
  const scale = window.devicePixelRatio || s.scale;
  return { x: s.physical.x + e.clientX * scale, y: s.physical.y + e.clientY * scale };
}

/**
 * Where a drag of a desktop item is now, in record space.
 *
 * `null` when there are no screens to map through (a probe page, a window with no
 * screen), in which case the caller keeps its own delta maths.
 */
export function dragPosition(ev: PointerEvent): { x: number; y: number } | null {
  const spot = pointerPhysical(ev);
  return spot ? physicalToVirtual(spot) : null;
}

/**
 * The point of the item the pointer grabbed, so it stays under that point.
 *
 * Deltas are exact while the pointer stays on one screen and wrong the moment it
 * crosses to another scale — and the screen-edge clamp that used to stop such a drag
 * dead is what turned "drag an icon to the other screen" into "merge it with whatever
 * sits at this screen's edge".
 */
export function dragGrab(ev: PointerEvent, x: number, y: number): { dx: number; dy: number } | null {
  const at = dragPosition(ev);
  return at ? { dx: at.x - x, dy: at.y - y } : null;
}

/** Is this record drawn by this window's screen? (true when there is no screen model) */
export function isOnMyScreen(x: number, y: number): boolean {
  return onMyScreen(x, y);
}

/**
 * A physical point → record space, through the screen it is physically on.
 *
 * With no screen list yet (a page that has just been reloaded, the first move of the
 * first drag) this falls back to *this window's own* screen rather than to nothing: a
 * point on this screen maps the same way either way, and returning nothing would drop
 * the drag back to the delta maths that the mapping exists to replace — measured, a
 * first drag after a reload crossed the boundary and left the record behind, drawn by
 * both windows at once.
 */
function physicalToVirtual(p: { x: number; y: number }): { x: number; y: number } | null {
  const list = screenList?.length ? screenList : area ? [area.screen] : [];
  if (!list.length) return null;
  const on = list.find(
    (s) =>
      p.x >= s.physical.x &&
      p.x < s.physical.x + s.physical.w &&
      p.y >= s.physical.y &&
      p.y < s.physical.y + s.physical.h,
  );
  if (!on) return null;
  return {
    x: on.logical.x + (p.x - on.physical.x) / on.scale,
    y: on.logical.y + (p.y - on.physical.y) / on.scale,
  };
}

/** The arrangement's space → this window's css px. */
export function toWindow(x: number, y: number): { x: number; y: number } {
  const origin = area?.screen.logical;
  return origin ? { x: x - origin.x, y: y - origin.y } : { x, y };
}

/** This window's css px → the arrangement's space. */
export function toRecords(x: number, y: number): { x: number; y: number } {
  const origin = area?.screen.logical;
  return origin ? { x: x + origin.x, y: y + origin.y } : { x, y };
}

/**
 * Is this point drawn by this window?
 *
 * True when nothing is known yet, so a page that has not managed to ask (or a build
 * where the command is missing) mounts everything rather than nothing.
 */
export function onMyScreen(x: number, y: number): boolean {
  const screen = area?.screen.logical;
  if (!screen) return true;
  return x >= screen.x && x < screen.x + screen.w && y >= screen.y && y < screen.y + screen.h;
}

/**
 * The monitors, in logical px, as the backend last reported them.
 *
 * One overlay window covers every screen (see `overlay_rect` in `lib.rs`), so a
 * widget's floor is the floor of *its* screen rather than of the whole arrangement.
 * The physics needs that answer synchronously, every frame, which is why the layout
 * is cached here and read by `monitorAt` instead of awaited per call.
 */
let monitors: MonitorArea[] = [];
let desktop: MonitorArea | null = null;

/**
 * Read the desktop rectangle and the monitor layout. Called once at mount, and
 * again whenever the display changes — a screen plugged in or unplugged is what
 * the resume path is for.
 */
export async function watchMonitors(): Promise<MonitorArea[]> {
  followMonitors();
  try {
    desktop = (await invoke<MonitorArea>("floaty_desktop_rect")) ?? desktop;
    monitors = (await invoke<MonitorArea[]>("floaty_monitors")) ?? monitors;
  } catch {
    /* keep whatever was known: a missing layout must not stop the desktop */
  }
  return monitors;
}

/**
 * Re-read the layout when Windows says the arrangement changed — a screen plugged in,
 * unplugged or re-scaled. The backend refits the overlay, brings stranded floaties
 * back, and emits this; a page that has ever asked about the layout is a page that
 * cares, so the subscription is made here rather than left to each caller.
 */
let monitorsWatched = false;

function followMonitors(): void {
  if (monitorsWatched) return;
  monitorsWatched = true;
  void listen("floaty-display-changed", () => {
    // this window's own screen may be a different one now, so the area is re-read
    // before the list it is measured against
    dropAreaCache();
    void myArea().then(() => watchMonitors());
  }).catch(() => undefined);
}

/**
 * The screen whose horizontal band contains `x`, or the whole desktop when the
 * layout is unknown. Monitors in a Windows arrangement are side by side in x, so
 * one comparison is the whole test — and on a single-monitor machine this is the
 * same rectangle `monitorArea()` returns.
 */
export function monitorAt(x: number): MonitorArea {
  const hit = monitors.find((m) => x >= m.x && x < m.x + m.w);
  if (hit) return hit;
  // This window's own screen is the better answer than the whole arrangement: an
  // overlay draws one screen, and a widget falling in it rests on that screen's floor.
  const mine = area?.screen.logical;
  return mine ?? monitorAreaSync();
}

/** The desktop rectangle without waiting for it (falls back to the window). */
export function monitorAreaSync(): MonitorArea {
  return (
    desktop ?? {
      x: 0,
      y: 0,
      w: window.innerWidth || 1920,
      h: window.innerHeight || 1080,
    }
  );
}

/** Current desktop area in logical px: every screen, not just the primary. */
export async function monitorArea(): Promise<MonitorArea> {
  if (isOverlayMode()) {
    // The screen this window covers, in record space. It used to be the window's own
    // size, which equalled the whole desktop only while there was one window for the
    // whole desktop.
    await myArea().catch(() => null);
    await watchMonitors().catch(() => []);
    return area?.screen.logical ?? monitorAreaSync();
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
  try {
    return await invoke<WidgetRecord | null>("floaty_get_record", { id }).then((r) => r ?? undefined);
  } catch (err) {
    void invoke("floaty_log", {
      msg: `[lib] loadRecord(${id}) failed: ${String(err)}`,
    }).catch(() => undefined);
    return undefined;
  }
}

export async function saveRecord(rec: WidgetRecord): Promise<void> {
  try {
    await invoke("floaty_save", { record: rec });
  } catch (err) {
    void invoke("floaty_log", {
      msg: `[lib] saveRecord(${rec.id}) failed: ${String(err)}`,
    }).catch(() => undefined);
  }
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
    // a slot stretched over the desktop is scaffolding, not the widget's place
    if (reservingSlots.has(rec.id)) return;
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
export interface PreventOverlapOptions {
  mon?: MonitorArea;
  gap?: number;
}

export interface PreventOverlapTarget {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  mon?: MonitorArea;
  gap?: number;
}

interface ShiftContext {
  curX: number;
  curY: number;
  w: number;
  h: number;
  gap: number;
  clampX: (val: number) => number;
  clampY: (val: number) => number;
}

function findShiftCandidate(
  colliding: OverlaySlot[],
  ctx: ShiftContext,
  collidesWithAny: (cx: number, cy: number) => boolean,
): { x: number; y: number } | null {
  const { curX, curY, w, h, gap, clampX, clampY } = ctx;
  type Candidate = { x: number; y: number; dist: number };
  const candidates: Candidate[] = [];

  for (const o of colliding) {
    const shifts = [
      { x: clampX(o.x + o.w + gap + 4), y: clampY(curY) },
      { x: clampX(o.x - w - gap - 4), y: clampY(curY) },
      { x: clampX(curX), y: clampY(o.y + o.h + gap + 4) },
      { x: clampX(curX), y: clampY(o.y - h - gap - 4) },
      { x: clampX(o.x + o.w + gap + 4), y: clampY(o.y) },
      { x: clampX(o.x - w - gap - 4), y: clampY(o.y) },
    ];
    for (const s of shifts) {
      if (!collidesWithAny(s.x, s.y)) {
        candidates.push({ x: s.x, y: s.y, dist: Math.hypot(s.x - curX, s.y - curY) });
      }
    }
  }

  if (candidates.length === 0) return null;
  candidates.sort((a, b) => a.dist - b.dist);
  return { x: candidates[0].x, y: candidates[0].y };
}

interface GridContext {
  curX: number;
  curY: number;
  minX: number;
  minY: number;
  clampX: (val: number) => number;
  clampY: (val: number) => number;
}

function findGridCandidate(
  ctx: GridContext,
  collidesWithAny: (cx: number, cy: number) => boolean,
): { x: number; y: number } | null {
  const { curX, curY, minX, minY, clampX, clampY } = ctx;
  const CELL_W = 100;
  const CELL_H = 116;
  const baseCol = Math.round((curX - minX) / CELL_W);
  const baseRow = Math.round((curY - minY) / CELL_H);

  for (let r = 1; r <= 15; r++) {
    for (let dc = -r; dc <= r; dc++) {
      for (let dr = -r; dr <= r; dr++) {
        if (Math.max(Math.abs(dc), Math.abs(dr)) !== r) continue;
        const gx = clampX(minX + (baseCol + dc) * CELL_W);
        const gy = clampY(minY + (baseRow + dr) * CELL_H);
        if (!collidesWithAny(gx, gy)) {
          return { x: gx, y: gy };
        }
      }
    }
  }
  return null;
}

/**
 * When a fresh widget drops on desktop without a stored position, check for
 * any existing widget in the overlay, nudging it to the nearest collision-free position.
 */
export function preventOverlap(
  targetOrId: string | PreventOverlapTarget,
  ...args: [optX?: number, optY?: number, optW?: number, optH?: number, options?: PreventOverlapOptions]
): { x: number; y: number } {
  const [optX, optY, optW, optH, options] = args;
  const isObj = typeof targetOrId !== "string";
  const id = isObj ? targetOrId.id : targetOrId;
  const x = isObj ? targetOrId.x : (optX ?? 0);
  const y = isObj ? targetOrId.y : (optY ?? 0);
  const w = isObj ? targetOrId.w : (optW ?? 0);
  const h = isObj ? targetOrId.h : (optH ?? 0);
  const mon = isObj ? targetOrId.mon : options?.mon;
  const gap = isObj ? (targetOrId.gap ?? 8) : (options?.gap ?? 8);

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

  const others: OverlaySlot[] = [];
  for (const slot of overlaySlots.values()) {
    if (slot.id !== id) others.push(slot);
  }

  const collidesWithAny = (cx: number, cy: number): boolean => {
    for (const o of others) {
      const ox = Math.min(cx + w, o.x + o.w) - Math.max(cx, o.x);
      const oy = Math.min(cy + h, o.y + o.h) - Math.max(cy, o.y);
      if (ox > 12 && oy > 12) return true;
    }
    return false;
  };

  if (!collidesWithAny(curX, curY)) {
    return { x: curX, y: curY };
  }

  const colliding = others.filter((o) => {
    const ox = Math.min(curX + w, o.x + o.w) - Math.max(curX, o.x);
    const oy = Math.min(curY + h, o.y + o.h) - Math.max(curY, o.y);
    return ox > 12 && oy > 12;
  });

  const shifted = findShiftCandidate(
    colliding,
    { curX, curY, w, h, gap, clampX, clampY },
    collidesWithAny,
  );
  if (shifted) return shifted;

  const gridPos = findGridCandidate(
    { curX, curY, minX, minY, clampX, clampY },
    collidesWithAny,
  );
  if (gridPos) return gridPos;

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
      // Capture on the body, not on the widget: the widget's slot is removed from the
      // DOM the moment the pointer crosses onto another screen (that window owns it
      // now), and capture on a removed element stops delivering moves — the drag would
      // freeze at the boundary. The body is always there.
      try {
        (el.ownerDocument?.body ?? el).setPointerCapture(e.pointerId);
      } catch {
        /* a browser without pointer capture: the drag still works inside the window */
      }
      const startX = slot.x;
      const startY = slot.y;
      const startSX = e.clientX;
      const startSY = e.clientY;
      let active = false;
      // The point of the floatie the pointer is holding, in record space. Placing it from
      // the pointer's own position is what survives the change of scale part way through
      // a drag: accumulating deltas in the window the drag began in pushed a floatie
      // dragged onto the second screen into the band between the two screens (1440 + px
      // past the edge, where the second screen begins at 1920) — a place no window draws,
      // so it disappeared for good.
      void screens();
      const at = pointerPhysical(e);
      const v = at && physicalToVirtual(at);
      const grab = v ? { dx: v.x - startX, dy: v.y - startY } : null;

      const onMove = (ev: PointerEvent) => {
        const dx = ev.clientX - startSX;
        const dy = ev.clientY - startSY;
        if (!active) {
          if (Math.abs(dx) < threshold && Math.abs(dy) < threshold) return;
          active = true;
          window.addEventListener("click", swallowClick, true);
          notifyDragging(true);
        }
        const spot = pointerPhysical(ev);
        const want = spot && physicalToVirtual(spot);
        if (want && grab) {
          const x = want.x - grab.dx;
          const y = want.y - grab.dy;
          // The backend is told on every move: it owns which screen the floatie is on and
          // hands it between windows as that changes, in both directions.
          void invoke("floaty_drag_to", { id, x, y }).catch(() => undefined);
          if (onMyScreen(x, y)) setWidgetPos(id, x, y);
          return;
        }
        // No screens to map through (or none under the pointer): the delta maths, which
        // is exact while the pointer stays on one screen.
        setWidgetPos(id, startX + dx, startY + dy);
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("pointercancel", onUp);
        try {
          (el.ownerDocument?.body ?? el).releasePointerCapture?.(e.pointerId);
        } catch {
          /* never captured */
        }
        // One last placement, so a drop just past the edge still lands on the screen the
        // pointer is on rather than on the last position the moves saw.
        if (active) {
          const spot = pointerPhysical({ clientX: e.clientX, clientY: e.clientY });
          const want = spot && physicalToVirtual(spot);
          if (want && grab) {
            void invoke("floaty_drag_to", {
              id,
              x: want.x - grab.dx,
              y: want.y - grab.dy,
            }).catch(() => undefined);
          }
        }
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

/**
 * Suffixes that say what a launcher *is*, not what it is called: the shortcut
 * files Windows puts on the desktop. Explorer hides these, so a tile that reads
 * "Zoom Workplace.lnk" is repeating an implementation detail — and since app
 * icons became real shortcuts, every one of them carries one.
 *
 * Deliberately a whitelist rather than "strip any extension": a file icon keeps
 * its suffix, because there it is the point (a .zip is not a .pdf).
 */
const LAUNCH_SUFFIX = /\.(lnk|url|exe|bat|cmd|com|msi|appref-ms|ps1|scr|jar)$/i;

/** The caption under an icon — `Arc.lnk` reads as `Arc`. The stored name is
 *  untouched: tooltips and the settings list still show the real file. */
export function displayName(name: string): string {
  const stripped = name.replace(LAUNCH_SUFFIX, "");
  return stripped || name;
}

/** Short label for a record: its name plus the path it points at — through
 *  `pathOf`, so a widget that happens to store its own `target` (the countdown's
 *  date) is not described as if that were a file. */
export function describeForConfirm(rec: WidgetRecord): string {
  const raw =
    typeof rec.data["name"] === "string" && (rec.data["name"] as string)
      ? (rec.data["name"] as string)
      : rec.id;
  const name = displayName(raw);
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
  single_click: string;
  double_click: string;
  live2d_root: string;
  files_root: string;
  disabled: string[];
  stay_on_desktop: boolean;
  /** launch floaty when the user signs in */
  start_on_boot: boolean;
  /** percentage of the desktop items that bob at all */
  animated_ratio: number;
  animation_mode: string;
  /** how far a resting icon rises, in px — the same travel in every mode */
  float_amplitude: number;
  /** seconds per bob */
  float_period: number;
  /** wave only: how much of the cycle separates one icon from the next, in % */
  float_spread: number;
  /** ask before removing a file, folder or widget */
  confirm_remove: boolean;
  /** visualizer sensitivity multiplier */
  viz_gain: number;
  /** visualizer redraw rate while audio plays */
  viz_fps: number;
  /** system monitor sample period in ms */
  sysmon_interval: number;
  /** the key that opens the launcher palette, e.g. "Ctrl+Alt+Space" */
  palette_shortcut: string;
}

export const DEFAULT_SETTINGS: FloatSettings = {
  pet_speed: 1,
  gravity: 2600,
  bounce: 0.45,
  single_click: "nothing",
  double_click: "launch",
  live2d_root: "",
  files_root: "",
  disabled: [],
  stay_on_desktop: true,
  start_on_boot: false,
  animated_ratio: 100,
  animation_mode: "wave",
  float_amplitude: 6,
  float_period: 10,
  float_spread: 10,
  confirm_remove: true,
  viz_gain: 1,
  viz_fps: 30,
  sysmon_interval: 1000,
  palette_shortcut: "Ctrl+Alt+Space",
};

/** A settings number, or the fallback when the field is missing or not one. */
function settingNum(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

/** The launcher cell and its gap: what "the next icon along" means to the eye. */
const CELL_W = 100;
const CELL_H = 120;

/**
 * Where an item sits in the ripple, read the way the eye reads a desktop —
 * left to right, top to bottom. A *place* is what a wave has to be ordered by:
 * the id it used to come from (`num % 6` of `app-17`, `folder-14`) scattered the
 * phases randomly across the screen, so even a stagger that applied would not
 * have read as a wave.
 */
function rippleIndex(at: { x: number; y: number }): number {
  const cols = Math.max(1, Math.ceil((window.innerWidth || 1920) / CELL_W));
  const col = clampNum(Math.round(at.x / CELL_W), 0, cols - 1);
  return Math.max(0, Math.round(at.y / CELL_H)) * cols + col;
}

/**
 * Applies the float settings to one desktop item: its mode, its travel and
 * period, and — in wave mode — where it sits in the ripple.
 *
 * `at` is the item's place on the desktop (omit it and the item takes the first
 * place, i.e. sync-like timing; the widgets pass it whenever they know it).
 *
 * The mode classes used to carry the stagger (`.phase-N`), which never worked:
 * an `animation` shorthand resets `animation-delay` to 0s, and the rule holding
 * the shorthand is a `.launcher.anim-wave.rest .tile` (specificity 0,4,0)
 * against `.phase-1 .tile` (0,2,0) — measured on twelve tiles, every one came
 * out `delay: 0s` and every phase group moved identically, so "wave" was
 * "sync" with a different name and the wave-spread slider did nothing at all.
 * The offset is `--bob-delay` now, read by the CSS inside the same rule as the
 * shorthand, and it is negative: a positive one leaves the far side of the
 * desktop standing still for tens of seconds before its turn comes.
 */
export function applyFloatieAnimation(
  wrap: HTMLElement,
  id: string,
  at?: { x: number; y: number },
): void {
  const s = currentSettings();
  const height = clampNum(settingNum(s.float_amplitude, 6), 0, 24);
  const period = clampNum(settingNum(s.float_period, 10), 2, 60);
  const ratio = clampNum(settingNum(s.animated_ratio, 100), 0, 100);
  const spread = clampNum(settingNum(s.float_spread, 10), 0, 100);
  const mode = s.animation_mode || "wave";

  // One knob, one meaning: `--bob-amp` is how far the icon rises, and every
  // mode's keyframes travel exactly that far. It used to be multiplied by a
  // second "float" slider as well, and the modes then scaled it again (wave
  // moved 1.22x the number, gentle 0.67x), so a px value only matched the
  // motion in the one mode nobody had chosen.
  wrap.style.setProperty("--bob-amp", `${height}px`);
  wrap.style.setProperty("--bob-cycle", `${period}s`);
  const ripple = mode === "wave" && at ? rippleIndex(at) : 0;
  wrap.style.setProperty("--bob-delay", `${((-ripple * spread) / 100) * period}s`);

  // The bob's step counts live in style.css as literal `steps(n)`: Chromium
  // silently drops `steps(var(--n))`, and a generated staircase with a stop per
  // step measured 5x the GPU of the compact form while looking the same.

  wrap.classList.remove("anim-wave", "anim-sync", "anim-gentle", "anim-static");

  // One line when this element's decision changes, so the log answers "did the
  // edit reach the widgets on screen" instead of leaving it to guesswork.
  const logDecision = (decision: string): void => {
    if (wrap.dataset.motionLogged === decision) return;
    wrap.dataset.motionLogged = decision;
    void invoke("floaty_log", { msg: `[motion] ${id} -> ${decision} (${describeMotion(s)})` }).catch(
      () => undefined,
    );
  };

  if (mode === "static" || ratio <= 0 || height <= 0) {
    wrap.classList.add("anim-static");
    logDecision("static");
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
    logDecision("static");
    return;
  }

  wrap.classList.add(`anim-${mode}`);
  logDecision(ripple > 0 ? `${mode} #${ripple}` : mode);
}

let settingsCache: FloatSettings = { ...DEFAULT_SETTINGS };
let settingsWatched = false;
/** True once the cache holds values the backend actually sent, not defaults. */
let settingsReady = false;
/** Everyone who has to re-apply something when the settings change. */
const settingsConsumers = new Set<() => void>();

export function currentSettings(): FloatSettings {
  return settingsCache;
}

/**
 * Re-run `cb` on every settings change, and once as soon as the real values are
 * in hand (immediately when they already are).
 *
 * This is the only way a widget should follow the settings. Registering a second
 * `listen("floaty-settings-changed", …)` instead — which is what the callers of
 * `applyFloatieAnimation` used to do — makes the result depend on *listener
 * registration order*: the handler that refreshes `settingsCache` and the handler
 * that reads it are two separate listeners on one event, so a widget can re-apply
 * with the values from before the change and never be told again. It also leaves
 * the first application wrong, because a page mounts its widgets before
 * `refreshSettings()` has answered and a pinned icon never runs another frame to
 * correct itself.
 */
export function onSettings(cb: () => void): void {
  settingsConsumers.add(cb);
  if (settingsReady) cb();
}

/** Fan out to every consumer, after the cache has been updated. */
function applySettings(): void {
  for (const cb of settingsConsumers) {
    try {
      cb();
    } catch {
      /* one bad consumer must not stop the others */
    }
  }
}

/**
 * The animation-relevant settings as one line, so the log can prove that an edit
 * reached the page that draws the widgets — which is the question that "it only
 * takes effect after a restart" always really is.
 */
function describeMotion(s: FloatSettings): string {
  return `ratio=${s.animated_ratio} mode=${s.animation_mode} height=${s.float_amplitude} period=${s.float_period} spread=${s.float_spread}`;
}
let lastMotion = "";

export async function refreshSettings(): Promise<void> {
  try {
    const s = await invoke<FloatSettings>("floaty_get_settings");
    settingsCache = { ...DEFAULT_SETTINGS, ...s };
    settingsReady = true;
  } catch {
    /* keep last */
  }
  applySettings();
}

/** Load once + stay live via backend change events. Safe to call per window. */
export function watchSettings(): void {
  if (settingsWatched) return;
  settingsWatched = true;
  void refreshSettings();
  listen<FloatSettings>("floaty-settings-changed", (e) => {
    settingsCache = { ...DEFAULT_SETTINGS, ...e.payload };
    settingsReady = true;
    const motion = describeMotion(settingsCache);
    if (motion !== lastMotion) {
      lastMotion = motion;
      void invoke("floaty_log", { msg: `[motion] ${appWin.label} settings: ${motion}` }).catch(
        () => undefined,
      );
    }
    // consumers run *after* the cache is updated: nobody reads a stale cache
    applySettings();
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

/**
 * Whether an icon still needs resolving.
 *
 * A stored icon is a file the backend owns, pointed at with an
 * `http://asset.localhost/…` url, so the only honest answer for one of those is
 * the backend's — `floaty_icon` looks for the file itself and re-resolves when
 * it is gone. A legacy data url can still be judged right here, by its PNG
 * header, which is what this used to do for every icon.
 */
export function iconIsMissing(url: string | undefined): boolean {
  if (!url || url === "none") return true;
  if (!url.startsWith("data:")) return false;
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
