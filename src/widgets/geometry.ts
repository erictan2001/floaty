import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { currentMonitor, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { onScreen, screenAt } from "./placement";
import { appWin, isOverlayMode } from "./windowState";
import { isOverlayDragging, overlaySlots, scheduleHitRectsUpdate } from "./slots";

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
export interface OverlayArea {
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
export interface ScreenInfo {
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
export function dragPosition(ev: {
  clientX: number;
  clientY: number;
}): { x: number; y: number } | null {
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
export function dragGrab(
  ev: { clientX: number; clientY: number },
  x: number,
  y: number,
): { dx: number; dy: number } | null {
  const at = dragPosition(ev);
  return at ? { dx: at.x - x, dy: at.y - y } : null;
}

/**
 * A physical point → record space, through the screen it is physically on.
 *
 * Exported because a drop from Explorer arrives as a physical position with no page
 * event behind it, and that is the same mapping a drag needs.
 *
 * With no screen list yet (a page that has just been reloaded, the first move of the
 * first drag) this falls back to *this window's own* screen rather than to nothing: a
 * point on this screen maps the same way either way, and returning nothing would drop
 * the drag back to the delta maths that the mapping exists to replace — measured, a
 * first drag after a reload crossed the boundary and left the record behind, drawn by
 * both windows at once.
 */
export function physicalToVirtual(p: { x: number; y: number }): { x: number; y: number } | null {
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
 * The test is the placement policy's (`placement.onScreen`); this is the adapter that hands
 * it the screen this window draws. True when nothing is known yet, so a page that has not
 * managed to ask (or a build where the command is missing) mounts everything rather than
 * nothing.
 */
export function onMyScreen(x: number, y: number): boolean {
  const screen = area?.screen.logical;
  if (!screen) return true;
  return onScreen(screen, x, y);
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
  followPresence();
  try {
    desktop = (await invoke<MonitorArea>("floaty_desktop_rect")) ?? desktop;
    monitors = (await invoke<MonitorArea[]>("floaty_monitors")) ?? monitors;
  } catch {
    /* keep whatever was known: a missing layout must not stop the desktop */
  }
  return monitors;
}

let monitorsWatched = false;
let presenceQuiet = false;
let presenceWatched = false;

const presenceListeners = new Set<(quiet: boolean) => void>();

export function onPresenceChange(cb: (quiet: boolean) => void): void {
  presenceListeners.add(cb);
}

export function presenceIsQuiet(): boolean {
  return presenceQuiet;
}

function followPresence(): void {
  if (presenceWatched) return;
  presenceWatched = true;
  void invoke("floaty_log", { msg: "[presence] page is following the machine's quiet" }).catch(
    () => undefined,
  );
  void listen<{
    quiet?: boolean;
    hidden?: boolean;
    reason?: string | null;
    machine?: { quiet?: boolean; hidden?: boolean };
  }>("floaty-presence", (e) => {
    const quiet = e.payload?.machine?.quiet ?? e.payload?.quiet === true;
    if (quiet === presenceQuiet) return;
    presenceQuiet = quiet;
    void invoke("floaty_log", {
      msg: `[presence] page quiet=${quiet} — ${quiet ? "motion and audio off" : "motion and audio back"}`,
    }).catch(() => undefined);
    for (const cb of presenceListeners) {
      try {
        cb(quiet);
      } catch {
        /* ignore consumer error */
      }
    }
    window.dispatchEvent(
      new CustomEvent("floaty-presence", {
        detail: { ...e.payload, quiet, screenQuiet: e.payload?.quiet === true },
      }),
    );
  }).catch(() => undefined);
}

function followMonitors(): void {
  if (monitorsWatched) return;
  monitorsWatched = true;
  void listen("floaty-display-changed", () => {
    dropAreaCache();
    void myArea().then(() => watchMonitors());
    followPresence();
  }).catch(() => undefined);
}

/**
 * The screen whose horizontal band contains `x`, or the whole desktop when the
 * layout is unknown. Monitors in a Windows arrangement are side by side in x, so
 * one comparison is the whole test — and on a single-monitor machine this is the
 * same rectangle `monitorArea()` returns.
 */
export function monitorAt(x: number): MonitorArea {
  const hit = screenAt(monitors, x);
  if (hit) return hit;
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

async function scaleFactor(): Promise<number> {
  try {
    const s = await appWin.scaleFactor();
    return s > 0 ? s : 1;
  } catch {
    return 1;
  }
}

/**
 * The session's hands: write a position or a size, and the window draws it.
 *
 * This is the drawing end only. The record is the position, and a stretch that is
 * scaffolding rather than a move is marked as one where the record is written —
 * `floatie(rec).place(x, y, { transient: true })` — so nothing here has to know.
 */
export function setWidgetPos(id: string, x: number, y: number, scale = 1): void {
  if (isOverlayMode()) {
    const slot = overlaySlots.get(id);
    if (slot) {
      slot.x = x;
      slot.y = y;
      const at = toWindow(x, y);
      slot.element.style.transform = `translate3d(${at.x}px, ${at.y}px, 0)`;
      if (!isOverlayDragging()) {
        scheduleHitRectsUpdate();
      }
    }
  } else {
    void appWin
      .setPosition(new PhysicalPosition(Math.round(x * scale), Math.round(y * scale)))
      .catch(() => undefined);
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
    void appWin
      .setSize(new PhysicalSize(Math.round(w * scale), Math.round(h * scale)))
      .catch(() => undefined);
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

/** Clamp helper used by the float-parameter plumbing (keeps NaN out of CSS). */
export function clampNum(v: number, min: number, max: number): number {
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
