import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  currentSettings,
  isOverlayMode,
  isPinned,
  isTopLayer,
  beatNow,
  startHeartbeat,
  appWin,
  monitorArea,
  overlaySlots,
  preventOverlap,
  registerOverlaySlot,
  saveRecord,
  scheduleHitRectsUpdate,
  unregisterOverlaySlot,
  watchSettings,
  type MonitorArea,
  type OverlaySlot,
  type WidgetRecord,
} from "./widgets/lib";
import { loadPlugins, pluginFor, pluginKinds, widgetApi } from "./widgets/plugin";
import { crossCheckPlugins, isDesktopItem, layoutPriorityFor, pluginSize } from "./widgets/pluginManifest";

/**
 * Resolves initial desktop layout to prevent icons and folders from overlapping.
 * Existing valid in-bounds positions are preserved; any widgets at (0, 0), offscreen,
 * or colliding with earlier widgets are assigned clean, staggered desktop grid coordinates.
 */
export function resolveOverlayLayout(list: WidgetRecord[], mon: MonitorArea, top: boolean): void {
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
    // Only this layer's widgets: the other layer's are laid out by its own page,
    // which is the one they are drawn in.
    if (isPinned(rec) !== top) continue;
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
  const top = isTopLayer();
  startHeartbeat(appWin.label);
  root.innerHTML = "";

  const canvas = document.createElement("div");
  canvas.id = "desktop-canvas";
  canvas.className = "overlay-canvas";
  root.append(canvas);

  const mountedSlots = new Set<string>();

  /* ---------- merge preview: the folder a drop would make ---------- */

  /**
   * Dragging one icon onto another is a real action — the backend folds the two
   * into a new folder in the pointed root, or puts the dragged item *inside* a
   * folder it lands on — and until now nothing said so until it had already
   * happened. Android's answer is the one people already know: the tile under
   * the finger becomes the folder it is about to be, its own icon shrinking into
   * one corner while the dragged one arrives in the other.
   *
   * The hit rule is copied from `floaty_dropped` on purpose (the dragged tile's
   * *centre* against every other desktop item's slot, each inflated by 12px), and
   * so is the tie-break, because a preview that disagrees with the drop would be
   * worse than none.
   */
  const MERGE_SLOP = 12;
  let mergeArmed: { target: string; dragged: string } | null = null;

  /** The tile a drop at this point would merge into, if any. */
  function mergeTargetAt(cx: number, cy: number, draggedId: string): OverlaySlot | null {
    // the backend walks its widget map, which serde keeps sorted by key, so two
    // pads covering the same centre resolve the same way here
    const candidates = [...overlaySlots.values()]
      .filter((s) => s.id !== draggedId && isDesktopItem(s.kind))
      .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
    for (const s of candidates) {
      const left = s.x - MERGE_SLOP;
      const top = s.y - MERGE_SLOP;
      const within =
        cx >= left &&
        cx < left + s.w + MERGE_SLOP * 2 &&
        cy >= top &&
        cy < top + s.h + MERGE_SLOP * 2;
      if (within) return s;
    }
    return null;
  }

  /**
   * Whatever the dragged tile is drawing — an app's icon, its letter, a folder's
   * glyph or first mini — cloned, so the preview shows the thing in hand rather
   * than a re-derived approximation of it.
   */
  function mergeGhostArt(dragged: OverlaySlot): Node | null {
    const pick = (sel: string): Element | null => dragged.element.querySelector(sel);
    const art =
      dragged.kind === "folder"
        ? (pick(".fminis img") ?? pick(".fminis .ffolder") ?? pick(".ffolder"))
        : (pick("img.tile-icon") ?? pick(".tile > span"));
    return art ? art.cloneNode(true) : null;
  }

  function disarmMerge(): void {
    if (!mergeArmed) return;
    const target = overlaySlots.get(mergeArmed.target);
    const dragged = overlaySlots.get(mergeArmed.dragged);
    const tile = target?.element.querySelector<HTMLElement>(".tile, .ftile");
    target?.element.classList.remove("merge-armed", "merge-two");
    target?.element.querySelector(".merge-ghost")?.remove();
    target?.element.querySelector(".merge-ring")?.remove();
    // the tile's own look comes back as it was: a letter tile paints a gradient
    // *inline*, which no stylesheet rule can override
    if (tile && tile.dataset.mergeBg !== undefined) {
      tile.style.background = tile.dataset.mergeBg;
      delete tile.dataset.mergeBg;
    }
    dragged?.element.querySelector(".launcher, .folder")?.classList.remove("merge-pulling");
    mergeArmed = null;
  }

  function armMerge(target: OverlaySlot, dragged: OverlaySlot): void {
    const tile = target.element.querySelector<HTMLElement>(".tile, .ftile");
    if (!tile) return;
    dragged.element.querySelector(".launcher, .folder")?.classList.add("merge-pulling");

    // the folder plate has to be the plate, not the icon's own gradient: appicon
    // paints letter tiles with an inline background, which beats the stylesheet
    if (tile.dataset.mergeBg === undefined) {
      tile.dataset.mergeBg = tile.style.background ?? "";
    }
    tile.style.background = "rgba(255, 255, 255, 0.3)";

    const ring = document.createElement("div");
    ring.className = "merge-ring";
    tile.appendChild(ring);

    const ghost = document.createElement("div");
    ghost.className = "merge-ghost";
    const art = mergeGhostArt(dragged);
    if (art) ghost.appendChild(art);

    // the incoming icon starts where the dragged tile is, so the eye follows it
    // into the folder instead of watching it appear
    const dx = dragged.x + dragged.w / 2 - (target.x + target.w / 2);
    const dy = dragged.y + dragged.h / 2 - (target.y + target.h / 2);
    ghost.style.setProperty("--from-x", `${Math.round(dx)}px`);
    ghost.style.setProperty("--from-y", `${Math.round(dy)}px`);

    if (target.kind === "folder") {
      // into the free cell of the folder's own 2x2 preview
      const used = tile.querySelectorAll(
        ".fminis img, .fminis .ffolder, .fminis .fmini-letter",
      ).length;
      ghost.classList.add(`slot-${used % 4}`);
    } else {
      // the tile itself becomes the two-icon folder preview, in the same
      // row-major order a folder's own mini grid uses (first icon top-left,
      // second top-right), so the plate matches the folder it is about to be
      target.element.classList.add("merge-two");
      ghost.classList.add("slot-1");
    }
    tile.appendChild(ghost);

    mergeArmed = { target: target.id, dragged: dragged.id };
    // arm on the next frame: inserted already-armed, the ghost would have no
    // starting position to animate from
    requestAnimationFrame(() => {
      if (mergeArmed?.target === target.id) target.element.classList.add("merge-armed");
    });
  }

  function refreshMerge(draggedId: string): void {
    const dragged = overlaySlots.get(draggedId);
    if (!dragged || !mountedSlots.has(draggedId)) {
      disarmMerge();
      return;
    }
    const target = mergeTargetAt(dragged.x + dragged.w / 2, dragged.y + dragged.h / 2, draggedId);
    if (!target) {
      disarmMerge();
      return;
    }
    if (mergeArmed && mergeArmed.target === target.id && mergeArmed.dragged === draggedId) return;
    disarmMerge();
    armMerge(target, dragged);
  }

  window.addEventListener("floaty-drag-move", (e) => {
    const detail = (e as CustomEvent<{ id?: string }>).detail;
    if (!detail?.id) return;
    refreshMerge(detail.id);
  });
  window.addEventListener("floaty-drag-end", () => disarmMerge());


  const mountWidget = (rec: WidgetRecord) => {
    if (mountedSlots.has(rec.id)) return;
    // Each layer draws only its own widgets: the desktop layer everything that
    // is not pinned, the always-on-top layer the ones that are. A widget is
    // mounted by exactly one of them (the backend tells both to reconcile when
    // the pin is toggled), which is what keeps a pinned widget from appearing in
    // the desktop layer as well.
    if (isPinned(rec) !== top) return;
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
    // a widget on its way out cannot be half of a preview
    if (mergeArmed && (mergeArmed.target === id || mergeArmed.dragged === id)) disarmMerge();
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
      const mine = list.filter((rec) => isPinned(rec) === top);
      invoke("floaty_log", {
        msg: `[overlay${top ? ":top" : ""}] init: plugins ${Math.round(tPlugins - t0)}ms, list(${list.length}) ${Math.round(
          tList - tPlugins,
        )}ms, mine(${mine.length}) monitor ${Math.round(tMonitor - tList)}ms, from page start ${Math.round(tMonitor)}ms`,
      }).catch(() => undefined);
      resolveOverlayLayout(list, mon, top);
      // Time each mount per kind: a reload's cost is dominated by whatever this
      // says, and the mount loop is synchronous, so the sum is the page's stall.
      const mountCost = new Map<string, number>();
      for (const rec of mine) {
        const t0 = performance.now();
        mountWidget(rec);
        mountCost.set(rec.kind, (mountCost.get(rec.kind) ?? 0) + (performance.now() - t0));
      }
      invoke("floaty_log", {
        msg:
          `[overlay${top ? ":top" : ""}] mount cost: ` +
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
      for (const rec of mine) {
        const { w, h } = pluginSize(rec);
        const slot = summary.get(rec.kind) ?? { n: 0, size: `${w}x${h}` };
        slot.n += 1;
        summary.set(rec.kind, slot);
      }
      invoke("floaty_log", {
        msg: `[overlay${top ? ":top" : ""}] mounted ${mountedSlots.size}/${mine.length} floaties — ${Array.from(summary)
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
      beatNow(appWin.label);
      invoke("floaty_log", { msg: `[overlay${top ? ":top" : ""}] hidden` }).catch(() => undefined);
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
    beatNow(appWin.label);
    invoke("floaty_log", {
      msg: `[overlay${top ? ":top" : ""}] visible again after ${Math.round(gap / 1000)}s`,
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
