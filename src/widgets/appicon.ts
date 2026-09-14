import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { PhysicalPosition } from "@tauri-apps/api/window";
import { stepBody, supportGone } from "./physics";
import {
  addPinMenu,
  appWin,
  applyFloatieAnimation,
  enforceDesktopLayer,
  currentSettings,
  displayName,
  iconIsMissing,
  isOverlayMode,
  loadRecord,
  logicalPos,
  monitorArea,
  notifyDragMove,
  notifyDragging,
  onSettings,
  overlaySlots,
  removeSelf,
  saveRecord,
  setLogicalPos,
  setWidgetPos,
  watchPluginEnabled,
  watchSettings,
  type MonitorArea,
  type WidgetRecord,
} from "./lib";
import type { FloatyPlugin, PluginRecord } from "./plugin";
import { isDesktopItem } from "./pluginManifest";

const WIN_W = 92;
const WIN_H = 112;

interface LayoutItem {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

const GRADIENTS = [
  ["#8b7bff", "#5b4bd6"],
  ["#4fc3f7", "#2470c7"],
  ["#5eead4", "#0d9488"],
  ["#fda4af", "#e11d63"],
  ["#fdba74", "#ea580c"],
  ["#fde047", "#ca8a04"],
];

function gradientFor(name: string): [string, string] {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return GRADIENTS[h % GRADIENTS.length] as [string, string];
}

export function mountLauncher(root: HTMLElement, id: string, kind: string = "app"): void {
  watchSettings();
  watchPluginEnabled(kind);
  const wrap = document.createElement("div");
  wrap.className = `launcher idle-hidden kind-${kind}`;
  wrap.innerHTML = `
    <button class="launcher-x" title="Remove">\u00d7</button>
    <div class="tile"><span></span></div>
    <div class="launcher-name"></div>
  `;
  root.append(wrap);
  const tile = wrap.querySelector<HTMLElement>(".tile")!;
  const letter = wrap.querySelector<HTMLElement>(".tile span")!;
  const nameEl = wrap.querySelector<HTMLElement>(".launcher-name")!;

  let rec: WidgetRecord | undefined;
  let x = 200;
  let y = 40;
  let vx = 0;
  let vy = 0;
  let settled = false;
  let dragging = false;
  let ready = false;
  let animating = false;
  let rafId: number | undefined;
  let layoutInterval: number | undefined;
  let mon: MonitorArea = { x: 0, y: 0, w: 1280, h: 800 };
  let scale = 1;
  let others: LayoutItem[] = [];
  let last = performance.now();
  let clickTimer: number | undefined;
  let suppressClickUntil = 0;
  let lastSavedX = -1;
  let lastSavedY = -1;
  let lastSavedPinned: boolean | undefined = undefined;
  // physics state that has to survive between frames (see widgets/physics.ts)
  let restTime = 0;
  let restingOn: string | null = null;
  let bounces = 0;
  let supportTimer: number | undefined;

  const syncAnim = () => {
    applyFloatieAnimation(wrap, id);
  };
  const syncFloat = syncAnim;
  // one consumer instead of a second settings listener: this runs after the
  // cached settings are updated, and once as soon as the first fetch lands
  onSettings(syncAnim);

  const squash = () => {
    tile.classList.remove("squash");
    void tile.offsetWidth;
    tile.classList.add("squash");
  };

  const saveSoon = () => {
    if (!rec) return;
    const curX = Math.round(x);
    const curY = Math.round(y);
    const curPinned = settled;
    if (curX === lastSavedX && curY === lastSavedY && curPinned === lastSavedPinned) {
      return;
    }
    lastSavedX = curX;
    lastSavedY = curY;
    lastSavedPinned = curPinned;
    rec.x = curX;
    rec.y = curY;
    rec.data["pinned"] = curPinned;
    void saveRecord(rec).catch(() => undefined);
  };

  const startLayoutPolling = () => {
    if (layoutInterval !== undefined) return;
    const fetchLayout = async () => {
      try {
        if (isOverlayMode()) {
          const items: LayoutItem[] = [];
          for (const slot of overlaySlots.values()) {
            // Everything the desktop is made of is solid — and the manifest is
            // what says so. A hand-written "app or folder" pair left *file*
            // icons out of the physics entirely, so an icon falling in from the
            // top (every unpinned one does, on arrival) dropped straight through
            // them, and through any stack of them, to the desktop below.
            if (slot.id !== id && isDesktopItem(slot.kind)) {
              items.push({ id: slot.id, x: slot.x, y: slot.y, w: slot.w, h: slot.h });
            }
          }
          others = items;
          return;
        }
        const all = await invoke<LayoutItem[]>("floaty_layout");
        others = all.filter((o) => o.id !== id);
      } catch {
        /* keep */
      }
    };
    void fetchLayout();
    layoutInterval = window.setInterval(fetchLayout, isOverlayMode() ? 200 : 500);
  };

  const stopLayoutPolling = () => {
    if (layoutInterval !== undefined) {
      window.clearInterval(layoutInterval);
      layoutInterval = undefined;
    }
    others = [];
  };

  const startAnimation = () => {
    if (animating) return;
    animating = true;
    // physics is running again, so the resting-support watchdog is not needed
    if (supportTimer !== undefined) {
      window.clearInterval(supportTimer);
      supportTimer = undefined;
    }
    last = performance.now();
    startLayoutPolling();
    void appWin.scaleFactor().then((s) => { if (s > 0) scale = s; }).catch(() => undefined);
    void monitorArea().then((m) => { mon = m; }).catch(() => undefined);
    rafId = requestAnimationFrame(frame);
  };

  const stopAnimation = () => {
    animating = false;
    stopLayoutPolling();
    if (rafId !== undefined) {
      cancelAnimationFrame(rafId);
      rafId = undefined;
    }
  };

  void (async () => {
    rec = await loadRecord(id);
    if (!rec) {
      wrap.remove();
      return;
    }
    const nm = typeof rec.data["name"] === "string" && rec.data["name"]
      ? (rec.data["name"] as string)
      : "app";
    const ch = (nm.trim()[0] ?? "?").toUpperCase();
    letter.textContent = ch;
    nameEl.textContent = displayName(nm);
    // the tooltip and the gradient keep the stored name: the colour of an
    // existing icon must not shift, and the real file is one hover away
    nameEl.title = nm;
    const [g1, g2] = gradientFor(nm);
    // bubble toggle: colored gradient tile on, bare floating icon off.
    // Must clear the inline background too — inline style beats the stylesheet.
    const setBubble = (on: boolean) => {
      tile.classList.toggle("real", !on);
      tile.style.background = on ? `linear-gradient(145deg, ${g1}, ${g2})` : "transparent";
      letter.style.display = on ? "" : "none";
    };
    setBubble(true);
    // real app icon when available (cached in the record, else resolved once
    // by the backend and persisted there); letter tile stays as the fallback
    const applyIcon = (url: string) => {
      setBubble(false);
      let img = tile.querySelector<HTMLImageElement>("img.tile-icon");
      if (!img) {
        img = document.createElement("img");
        img.className = "tile-icon";
        img.draggable = false;
        img.alt = "";
        tile.prepend(img);
      }
      img.src = url;
    };
    const cachedIcon = typeof rec.data["icon"] === "string" ? (rec.data["icon"] as string) : "";
    if (cachedIcon && cachedIcon !== "none") {
      applyIcon(cachedIcon);
    }
    // Only a *missing* icon is worth a resolver round-trip. A 32px icon is
    // upgraded by the backend's background pass — re-resolving on every mount
    // used to spawn PowerShell per widget and still came back 32px for most
    // file types, so the tile looked like it was stuck on an old icon.
    if (iconIsMissing(cachedIcon)) {
      invoke<string>("floaty_icon", { id: rec.id })
        .then((hiRes) => {
          if (hiRes && hiRes !== "none") {
            if (rec) rec.data["icon"] = hiRes;
            applyIcon(hiRes);
          }
        })
        .catch(() => undefined);
    }

    listen<string>("floaty-icon-refreshed", (e) => {
      if (e.payload === id) {
        void loadRecord(id).then((r) => {
          if (r && typeof r.data["icon"] === "string" && r.data["icon"] && r.data["icon"] !== "none") {
            if (rec) rec.data["icon"] = r.data["icon"];
            applyIcon(r.data["icon"] as string);
          }
        });
      }
    }).catch(() => undefined);
    // desktop layer (with the builder flag): icons live under real apps. In an
    // overlay the window's z-order is the layer's, so this only applies when a
    // widget has a window of its own.
    enforceDesktopLayer();
    addPinMenu(wrap, () => rec);
    wrap.classList.remove("idle-hidden");

    try {
      const p = await logicalPos(id);
      x = p.x;
      y = p.y;
    } catch {
      x = rec.x;
      y = rec.y;
    }
    ready = true;
    try {
      mon = await monitorArea();
    } catch {
      /* defaults */
    }
    try {
      const s = await appWin.scaleFactor();
      if (s > 0) scale = s;
    } catch {
      /* keep */
    }
    // restore pinned state: pinned icons stay where they were, the rest
    // fall in from the top on arrival as before
    vy = 0;
    settled = rec.data["pinned"] === true;
    syncFloat();
    if (settled) {
      wrap.classList.add("rest");
    } else {
      startAnimation();
    }

    window.addEventListener("beforeunload", () => void saveSoon());
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) {
        void saveSoon();
        if (animating) stopAnimation();
      } else if (!settled) {
        startAnimation();
      }
    });
  })();

  wrap.querySelector(".launcher-x")?.addEventListener("pointerdown", (e) => e.stopPropagation());
  wrap.querySelector(".launcher-x")?.addEventListener("click", (e) => {
    e.stopPropagation();
    if (rec) void removeSelf(rec);
  });

  // Manual drag (same reason as the pet): an OS-level startDragging on every
  // press swallows the click sequence, so dblclick-to-launch never fires.
  // The window follows the cursor and x/y stay exact — no re-read needed.
  wrap.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || dragging) return;
    e.stopPropagation();
    // NOTE: no preventDefault() — canceling pointerdown kills click/dblclick.
    dragging = true;
    notifyDragging(true);
    wrap.classList.add("held");
    squash();
    // capture lazily on first real movement (see pet.ts: eager capture eats taps)
    let captured = false;
    const isOverlay = isOverlayMode();
    const startX = x;
    const startY = y;
    const startSX = isOverlay ? e.clientX : e.screenX;
    const startSY = isOverlay ? e.clientY : e.screenY;
    let moved = false;
    const onMove = (ev: PointerEvent) => {
      const curSX = isOverlay ? ev.clientX : ev.screenX;
      const curSY = isOverlay ? ev.clientY : ev.screenY;
      x = startX + (curSX - startSX);
      y = startY + (curSY - startSY);
      if (isOverlay) {
        const monW = window.innerWidth || 1920;
        const monH = window.innerHeight || 1080;
        x = Math.max(0, Math.min(monW - WIN_W, x));
        y = Math.max(0, Math.min(monH - WIN_H, y));
      } else {
        if (x < mon.x) x = mon.x;
        if (x > mon.x + mon.w - WIN_W) x = mon.x + mon.w - WIN_W;
        if (y < mon.y) y = mon.y;
        if (y > mon.y + mon.h - WIN_H) y = mon.y + mon.h - WIN_H;
      }
      if (Math.hypot(curSX - startSX, curSY - startSY) > 4) {
        moved = true;
        suppressClickUntil = performance.now() + 300;
        if (!captured) {
          captured = true;
          try {
            wrap.setPointerCapture(e.pointerId);
          } catch {
            /* capture unsupported — window-level listeners still cover the drag */
          }
        }
      }
      setWidgetPos(id, x, y, scale);
      // tell the overlay where we are, so it can show the merge this hover
      // would run (nothing listens in one-window-per-widget mode)
      notifyDragMove(id, x, y);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      try {
        if (wrap.hasPointerCapture(e.pointerId)) wrap.releasePointerCapture(e.pointerId);
      } catch {
        /* ignore */
      }
      wrap.classList.remove("held");
      if (moved) {
        // was a real drag: pin exactly where dropped, no fall
        vx = 0;
        vy = 0;
        settled = true;
        restingOn = null;
        restTime = 0;
        bounces = 0;
        syncFloat();
        wrap.classList.add("rest");
        stopAnimation();

        void (async () => {
          try {
            // Group mode: backend merges us into whatever icon/folder we landed on
            const merged = await invoke<string | null>("floaty_dropped", {
              id,
              x: Math.round(x),
              y: Math.round(y),
            });
            if (merged) {
              return; // Merged into folder; slot will be unmounted
            }
          } catch {
            /* ignore */
          }

          // Not merged: keep exact dropped position and persist
          void saveSoon();
        })();
      }
      // a tap leaves `settled` untouched — click/dblclick decide what happens
      dragging = false;
      notifyDragging(false);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  });

  const doDrop = () => {
    settled = false;
    restTime = 0;
    bounces = 0;
    restingOn = null;
    syncFloat();
    wrap.classList.remove("rest");
    vy = 0;
    vx += (Math.random() - 0.5) * 60;
    squash();
    startAnimation();
  };
  const doHop = () => {
    settled = false;
    restTime = 0;
    bounces = 0;
    restingOn = null;
    syncFloat();
    wrap.classList.remove("rest");
    vy = -420;
    vx += (Math.random() - 0.5) * 120;
    squash();
    startAnimation();
  };
  const doPin = () => {
    vx = 0;
    vy = 0;
    settled = true;
    restingOn = null;
    restTime = 0;
    syncFloat();
    wrap.classList.add("rest");
    void saveSoon();
    stopAnimation();
  };
  wrap.addEventListener("click", (e) => {
    e.stopPropagation();
    if (performance.now() < suppressClickUntil) return; // was a drag, not a tap
    // single-click action from settings (cancelled if a double-click follows)
    window.clearTimeout(clickTimer);
    clickTimer = window.setTimeout(() => {
      const a = currentSettings().single_click;
      if (a === "hop") doHop();
      else if (a === "nothing") { /* stay put */ }
      else doDrop();
    }, 260);
  });
  wrap.addEventListener("dblclick", (e) => {
    e.stopPropagation();
    window.clearTimeout(clickTimer);
    const a = currentSettings().double_click;
    if (a === "drop") {
      doDrop();
      return;
    }
    if (a === "nothing") {
      doPin();
      return;
    }
    if (!rec) return;
    // launch without dropping: pin in place even if it was falling
    doPin();
    tile.classList.add("launching");
    window.setTimeout(() => tile.classList.remove("launching"), 600);
    invoke("floaty_launch", { id: rec.id }).catch(() => {
      tile.classList.remove("launching");
      tile.classList.add("shake");
      window.setTimeout(() => tile.classList.remove("shake"), 500);
    });
  });
  wrap.addEventListener("contextmenu", (e) => e.preventDefault());

  /** Come to rest: pin in place, persist, and stop burning frames on it. */
  const goToRest = (on: string | null): void => {
    settled = true;
    restingOn = on;
    syncFloat();
    wrap.classList.add("rest");
    void saveSoon();
    stopAnimation();
    // An icon resting on another one is only stable while that one stays put.
    if (on) watchSupport();
  };

  /** Cheap poll for a resting icon: fall again when its support moves away. */
  const watchSupport = (): void => {
    if (supportTimer !== undefined) return;
    startLayoutPolling();
    supportTimer = window.setInterval(() => {
      if (!settled || dragging) return;
      // no layout data yet (the fetch is async): that is not "support gone"
      if (others.length === 0) return;
      if (supportGone({ x, y, w: WIN_W, h: WIN_H }, others)) wake();
    }, 250);
  };

  const stopSupportWatch = (): void => {
    if (supportTimer !== undefined) {
      window.clearInterval(supportTimer);
      supportTimer = undefined;
    }
  };

  /** Support disappeared (or the user moved things): run physics again. */
  const wake = (): void => {
    stopSupportWatch();
    settled = false;
    restingOn = null;
    restTime = 0;
    bounces = 0;
    syncFloat();
    wrap.classList.remove("rest");
    startAnimation();
  };

  const frame = (now: number) => {
    if (!animating) return;
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    // pinned icons skip physics entirely — they stay where dropped.
    // Never drive the window before the stored position loads (ready),
    // or every icon first jumps to default coordinates and bunches up.
    if (ready && !dragging && !settled && !document.hidden) {
      const out = stepBody({
        body: { x, y, vx, vy, w: WIN_W, h: WIN_H, restTime, restingOn, bounces },
        gravity: currentSettings().gravity,
        restitution: currentSettings().bounce,
        bounds: mon,
        others,
        dt,
      });
      x = out.x;
      y = out.y;
      vx = out.vx;
      vy = out.vy;
      restTime = out.restTime;
      bounces = out.bounces;
      if (out.impacts > 0) squash();
      if (out.settled) {
        goToRest(out.restingOn);
        return;
      }
      restingOn = out.restingOn;
      wrap.classList.remove("rest");
      setWidgetPos(id, x, y, scale);
    }
    if (animating) {
      rafId = requestAnimationFrame(frame);
    }
  };
}

export const appPlugin: FloatyPlugin = {
  kind: "app",
  mount: (root, id) => mountLauncher(root, id, "app"),
  describe: (rec: PluginRecord) => {
    const n = rec.data["name"];
    return typeof n === "string" && n ? n : undefined;
  },
  // No settings of its own: how an icon falls, floats and answers a click is
  // the same for every desktop item, so it lives in the Motion tab (see
  // `settings/params.ts` for the rows). App icons cannot be added from a
  // button either — they are floated from the Apps tab or dragged in.
};

/**
 * Loose files from the desktop root: same gravity/float behaviour as an app
 * launcher, but the id/double-click opens the document with its default app
 * instead of being spawned as a program (the backend classifies the kind).
 */
export const filePlugin: FloatyPlugin = {
  kind: "file",
  mount: (root, id) => mountLauncher(root, id, "file"),
  describe: (rec: PluginRecord) => {
    const n = rec.data["name"];
    return typeof n === "string" && n ? n : undefined;
  },
};
