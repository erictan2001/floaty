import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  currentSettings,
  isOverlayMode,
  beatNow,
  startHeartbeat,
  monitorArea,
  preventOverlap,
  registerOverlaySlot,
  saveRecord,
  scheduleHitRectsUpdate,
  unregisterOverlaySlot,
  watchSettings,
  type MonitorArea,
  type WidgetRecord,
} from "./widgets/lib";
import { loadPlugins, pluginFor, pluginKinds, widgetApi } from "./widgets/plugin";
import { crossCheckPlugins, layoutPriorityFor, pluginSize } from "./widgets/pluginManifest";

/**
 * Resolves initial desktop layout to prevent icons and folders from overlapping.
 * Existing valid in-bounds positions are preserved; any widgets at (0, 0), offscreen,
 * or colliding with earlier widgets are assigned clean, staggered desktop grid coordinates.
 */
export function resolveOverlayLayout(list: WidgetRecord[], mon: MonitorArea): void {
  const screenW = mon.w > 0 ? mon.w : (window.innerWidth || 1920);
  const screenH = mon.h > 0 ? mon.h : (window.innerHeight || 1080);

  interface PlacedItem {
    id: string;
    x: number;
    y: number;
    w: number;
    h: number;
  }
  const placed: PlacedItem[] = [];
  const needsRelocation: WidgetRecord[] = [];

  const sorted = [...list].sort(
    (a, b) => layoutPriorityFor(a.kind) - layoutPriorityFor(b.kind),
  );

  for (const rec of sorted) {
    const { w, h } = pluginSize(rec);
    const hasPos = rec.x > 10 || rec.y > 10;
    const inBounds =
      rec.x >= 16 &&
      rec.y >= 16 &&
      rec.x + w <= screenW - 16 &&
      rec.y + h <= screenH - 16;

    let collides = false;
    if (hasPos && inBounds) {
      for (const p of placed) {
        const ox = Math.min(rec.x + w, p.x + p.w) - Math.max(rec.x, p.x);
        const oy = Math.min(rec.y + h, p.y + p.h) - Math.max(rec.y, p.y);
        if (ox > 40 && oy > 40) {
          collides = true;
          break;
        }
      }
    }

    if (hasPos && inBounds && !collides) {
      placed.push({ id: rec.id, x: rec.x, y: rec.y, w, h });
    } else {
      needsRelocation.push(rec);
    }
  }

  // Lay out needsRelocation across the desktop grid columns
  const MARGIN_LEFT = 24;
  const MARGIN_TOP = 24;
  const MARGIN_BOTTOM = 72;
  const CELL_W = 100;
  const CELL_H = 116;

  const usableH = Math.max(200, screenH - MARGIN_TOP - MARGIN_BOTTOM);
  const rowsPerCol = Math.max(1, Math.floor(usableH / CELL_H));

  let gridIndex = 0;
  for (const rec of needsRelocation) {
    const { w, h } = pluginSize(rec);
    // Clamp into the screen *before* testing for room. The clamp used to run
    // after, so any widget wider than a grid cell (a 240px plugin, a 392px
    // live2d) was pushed back onto the icon grid and ended up hidden behind the
    // tiles that were already there.
    let placedAt: { x: number; y: number } | undefined;
    for (let tries = 0; tries < 500 && !placedAt; tries++) {
      const col = Math.floor(gridIndex / rowsPerCol);
      const row = gridIndex % rowsPerCol;
      gridIndex++;
      const gx = Math.max(MARGIN_LEFT, Math.min(MARGIN_LEFT + col * CELL_W, screenW - w - 16));
      const gy = Math.max(MARGIN_TOP, Math.min(MARGIN_TOP + row * CELL_H, screenH - h - 16));

      const collides = placed.some((p) => {
        const ox = Math.min(gx + w, p.x + p.w) - Math.max(gx, p.x);
        const oy = Math.min(gy + h, p.y + p.h) - Math.max(gy, p.y);
        return ox > 12 && oy > 12;
      });
      if (!collides) placedAt = { x: gx, y: gy };
    }
    // a full desktop still gets the widget: last clamped spot rather than an
    // endless search
    placedAt ??= {
      x: Math.max(MARGIN_LEFT, Math.min(MARGIN_LEFT, screenW - w - 16)),
      y: Math.max(MARGIN_TOP, Math.min(MARGIN_TOP, screenH - h - 16)),
    };
    rec.x = placedAt.x;
    rec.y = placedAt.y;
    placed.push({ id: rec.id, x: rec.x, y: rec.y, w, h });
    void saveRecord(rec);
  }
}

export function mountOverlay(root: HTMLElement): void {
  watchSettings();
  startHeartbeat("desktop-overlay");
  root.innerHTML = "";

  const canvas = document.createElement("div");
  canvas.id = "desktop-canvas";
  canvas.className = "overlay-canvas";
  root.append(canvas);

  const mountedSlots = new Set<string>();

  const mountWidget = (rec: WidgetRecord) => {
    if (mountedSlots.has(rec.id)) return;
    const s = currentSettings();
    if (s.disabled && s.disabled.includes(rec.kind)) return;

    const plugin = pluginFor(rec.kind);
    if (!plugin) return;

    const { w, h } = pluginSize(rec);
    const slot = document.createElement("div");
    slot.className = "overlay-slot";
    slot.id = `slot-${rec.id}`;
    slot.dataset.id = rec.id;
    slot.dataset.kind = rec.kind;
    slot.style.transform = `translate3d(${rec.x}px, ${rec.y}px, 0)`;
    slot.style.width = `${w}px`;
    slot.style.height = `${h}px`;

    canvas.append(slot);
    mountedSlots.add(rec.id);

    registerOverlaySlot({
      id: rec.id,
      kind: rec.kind,
      x: rec.x,
      y: rec.y,
      w,
      h,
      element: slot,
    });

    // mount may be async (installed plugins load their record first), so the
    // rejection has to be caught on the promise: a throw alone used to lose the
    // only clue a third-party plugin left behind.
    try {
      void Promise.resolve(plugin.mount(slot, rec.id, widgetApi)).catch((e: unknown) => {
        invoke("floaty_log", { msg: `[overlay] mount ${rec.id} failed: ${String(e)}` }).catch(
          () => undefined,
        );
      });
    } catch (e) {
      invoke("floaty_log", { msg: `[overlay] failed to mount ${rec.id}: ${String(e)}` }).catch(
        () => undefined,
      );
    }
  };

  const unmountWidget = (id: string) => {
    if (!mountedSlots.has(id)) return;
    mountedSlots.delete(id);
    unregisterOverlaySlot(id);
    const slot = document.getElementById(`slot-${id}`);
    if (slot) slot.remove();
  };

  // Initial load: the manifest first, so every slot is sized from the one place
  // the backend keeps sizes and no widget is laid out with a guess.
  void (async () => {
    try {
      const t0 = performance.now();
      await loadPlugins();
      const tPlugins = performance.now();
      crossCheckPlugins(pluginKinds());
      const list = await invoke<WidgetRecord[]>("floaty_list");
      const tList = performance.now();
      const mon = await monitorArea();
      const tMonitor = performance.now();
      invoke("floaty_log", {
        msg: `[overlay] init: plugins ${Math.round(tPlugins - t0)}ms, list(${list.length}) ${Math.round(
          tList - tPlugins,
        )}ms, monitor ${Math.round(tMonitor - tList)}ms, from page start ${Math.round(tMonitor)}ms`,
      }).catch(() => undefined);
      resolveOverlayLayout(list, mon);
      // Time each mount per kind: a reload's cost is dominated by whatever this
      // says, and the mount loop is synchronous, so the sum is the page's stall.
      const mountCost = new Map<string, number>();
      for (const rec of list) {
        const t0 = performance.now();
        mountWidget(rec);
        mountCost.set(rec.kind, (mountCost.get(rec.kind) ?? 0) + (performance.now() - t0));
      }
      invoke("floaty_log", {
        msg:
          "[overlay] mount cost: " +
          Array.from(mountCost)
            .sort((a, b) => b[1] - a[1])
            .map(([k, ms]) => `${k} ${Math.round(ms)}ms`)
            .join(", "),
      }).catch(() => undefined);
      scheduleHitRectsUpdate();
      // One line per launch: which kinds mounted, at which size, and where the
      // sizes came from. If the manifest did not arrive, every kind would be
      // missing here and the slots would be the 100x100 fallback.
      const summary = new Map<string, { n: number; size: string }>();
      for (const rec of list) {
        const { w, h } = pluginSize(rec);
        const slot = summary.get(rec.kind) ?? { n: 0, size: `${w}x${h}` };
        slot.n += 1;
        summary.set(rec.kind, slot);
      }
      invoke("floaty_log", {
        msg: `[overlay] mounted ${mountedSlots.size}/${list.length} floaties — ${Array.from(summary)
          .map(([kind, s]) => `${kind} ${s.n}x${s.size}`)
          .join(", ")}`,
      }).catch(() => undefined);
    } catch (e) {
      invoke("floaty_log", { msg: `[overlay] initial load failed: ${String(e)}` }).catch(
        () => undefined,
      );
    }
  })();

  // Listen for backend widget additions and removals
  listen<WidgetRecord>("floaty-widget-added", (e) => {
    if (e.payload) {
      const rec = e.payload;
      if (rec.data["pinned"] !== true) {
        const { w, h } = pluginSize(rec);
        const safe = preventOverlap(rec.id, rec.x, rec.y, w, h);
        if (safe.x !== rec.x || safe.y !== rec.y) {
          rec.x = safe.x;
          rec.y = safe.y;
          void saveRecord(rec);
        }
      }
      mountWidget(rec);
      scheduleHitRectsUpdate();
    }
  }).catch(() => undefined);

  // the backend renamed/changed a widget on disk: remount it so the label,
  // icon and hit rects match the new name
  listen<WidgetRecord>("floaty-widget-updated", (e) => {
    const rec = e.payload;
    if (!rec) return;
    unmountWidget(rec.id);
    mountWidget(rec);
    scheduleHitRectsUpdate();
  }).catch(() => undefined);

  listen<string>("floaty-widget-removed", (e) => {
    if (e.payload) {
      unmountWidget(e.payload);
      scheduleHitRectsUpdate();
    }
  }).catch(() => undefined);

  listen<Array<{ id: string; enabled: boolean }>>("floaty-plugins-changed", (e) => {
    const changes = e.payload || [];
    for (const p of changes) {
      if (!p.enabled) {
        // remove all widgets of this kind
        const toRemove: string[] = [];
        for (const slotId of mountedSlots) {
          const el = document.getElementById(`slot-${slotId}`);
          if (el && el.dataset.kind === p.id) {
            toRemove.push(slotId);
          }
        }
        toRemove.forEach(unmountWidget);
      } else {
        // re-mount widgets of this kind
        void invoke<WidgetRecord[]>("floaty_list").then((list) => {
          for (const rec of list) {
            if (rec.kind === p.id) {
              mountWidget(rec);
            }
          }
          scheduleHitRectsUpdate();
        });
      }
    }
  }).catch(() => undefined);

  // MutationObserver to detect DOM changes (popups, folder expansion) and sync hit rects
  const observer = new MutationObserver(() => {
    scheduleHitRectsUpdate();
  });
  observer.observe(canvas, { childList: true, subtree: true, attributes: true, attributeFilter: ["style", "class"] });

  // When window loses focus or user clicks outside, dismiss any menus
  window.addEventListener("blur", () => {
    const popups = document.querySelectorAll(".pin-menu, .model-menu");
    if (popups.length > 0) {
      popups.forEach((m) => m.remove());
      scheduleHitRectsUpdate();
    }
  });

  // Sleep/standby evidence: the backend does the recovery (it gets the display
  // power setting), but a hidden/visible transition is rare and is the only
  // record of what the page thought while the lid was shut.
  let hiddenSince = 0;
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      hiddenSince = Date.now();
      // Tell the backend now: it knows whether the display is actually on, and a
      // hidden page while the display is on means Chromium's occlusion verdict is
      // stale and this page's timers are about to be throttled.
      beatNow("desktop-overlay");
      invoke("floaty_log", { msg: "[overlay] hidden" }).catch(() => undefined);
      return;
    }
    const gap = hiddenSince ? Date.now() - hiddenSince : 0;
    hiddenSince = 0;
    // Coming back from hidden is exactly when the webview's surface can still be
    // holding its very first (white) frame in the regions this page paints
    // nothing into — the translucent tiles on top of it then show it as white
    // bands. Detaching the canvas for one frame makes Chromium repaint the whole
    // surface instead of only the damage it knows about.
    forceRepaint();
    beatNow("desktop-overlay");
    invoke("floaty_log", {
      msg: `[overlay] visible again after ${Math.round(gap / 1000)}s`,
    }).catch(() => undefined);
  });

  // Global escape key to collapse open folders / close menus
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      document.querySelectorAll(".pin-menu, .model-menu").forEach((m) => m.remove());
      document.querySelectorAll(".folder.open .fshut").forEach((b) => (b as HTMLElement).click());
      scheduleHitRectsUpdate();
    }
  });
}

/**
 * Make the whole surface paint again.
 *
 * A transparent WebView2 starts life painting a white frame of its own, and
 * Chromium only repaints the damage it is told about. Anything this page never
 * covers can therefore keep that white indefinitely — which reads as solid white
 * bands wherever a translucent tile sits over it. Hiding the canvas for one
 * frame invalidates the whole layer, so the next frame replaces it.
 */
export function forceRepaint(): void {
  const canvas = document.querySelector(".overlay-canvas");
  if (!(canvas instanceof HTMLElement)) return;
  const prev = canvas.style.visibility;
  canvas.style.visibility = "hidden";
  void canvas.offsetHeight;
  canvas.style.visibility = prev;
}
