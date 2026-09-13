import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  currentSettings,
  isOverlayMode,
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
import { plugins } from "./widgets/plugin";

function getInitialWidgetSize(rec: WidgetRecord): { w: number; h: number } {
  switch (rec.kind) {
    case "app":
    case "file":
      return { w: 92, h: 112 };
    case "folder":
      return { w: 92, h: 112 };
    case "pet":
      return { w: 170, h: 170 };
    case "live2d": {
      // the zoom sizes the box (clamped to the desktop in live2d.ts)
      const w = typeof rec.data["w"] === "number" ? (rec.data["w"] as number) : 300;
      const h = typeof rec.data["h"] === "number" ? (rec.data["h"] as number) : 400;
      return { w, h };
    }
    case "visualizer": {
      const w = typeof rec.data["w"] === "number" ? (rec.data["w"] as number) : 280;
      const h = typeof rec.data["h"] === "number" ? (rec.data["h"] as number) : 130;
      return { w, h };
    }
    case "sysmon": {
      const w = typeof rec.data["w"] === "number" ? (rec.data["w"] as number) : 250;
      const h = typeof rec.data["h"] === "number" ? (rec.data["h"] as number) : 170;
      return { w, h };
    }
    case "note": {
      const w = typeof rec.data["w"] === "number" ? (rec.data["w"] as number) : 300;
      const h = typeof rec.data["h"] === "number" ? (rec.data["h"] as number) : 330;
      return { w, h };
    }
    case "clock": {
      const w = typeof rec.data["w"] === "number" ? (rec.data["w"] as number) : 250;
      const h = typeof rec.data["h"] === "number" ? (rec.data["h"] as number) : 330;
      return { w, h };
    }
    default:
      return { w: 100, h: 100 };
  }
}

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

  // Special/custom widgets take priority to preserve user placement
  const priorityOrder = (kind: string): number => {
    switch (kind) {
      case "live2d":
        return 1;
      case "note":
        return 2;
      case "clock":
        return 3;
      case "pet":
        return 4;
      // widgets the user places by hand outrank the icon/folder grid, so a
      // dragged position is not reshuffled on the next launch
      case "visualizer":
      case "sysmon":
        return 5;
      default:
        return 10;
    }
  };

  const sorted = [...list].sort((a, b) => priorityOrder(a.kind) - priorityOrder(b.kind));

  for (const rec of sorted) {
    const { w, h } = getInitialWidgetSize(rec);
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
    const { w, h } = getInitialWidgetSize(rec);
    while (true) {
      const col = Math.floor(gridIndex / rowsPerCol);
      const row = gridIndex % rowsPerCol;
      const gx = MARGIN_LEFT + col * CELL_W;
      const gy = MARGIN_TOP + row * CELL_H;
      gridIndex++;

      const collides = placed.some((p) => {
        const ox = Math.min(gx + w, p.x + p.w) - Math.max(gx, p.x);
        const oy = Math.min(gy + h, p.y + p.h) - Math.max(gy, p.y);
        return ox > 12 && oy > 12;
      });

      if (!collides) {
        // clamp into the screen: a long grid column used to park floaties past
        // the right/bottom edge, where they were only half visible
        rec.x = Math.max(MARGIN_LEFT, Math.min(gx, screenW - w - 16));
        rec.y = Math.max(MARGIN_TOP, Math.min(gy, screenH - h - 16));
        placed.push({ id: rec.id, x: rec.x, y: rec.y, w, h });
        void saveRecord(rec);
        break;
      }
    }
  }
}

export function mountOverlay(root: HTMLElement): void {
  watchSettings();
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

    const plugin = plugins.find((p) => p.kind === rec.kind);
    if (!plugin) return;

    const { w, h } = getInitialWidgetSize(rec);
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

    try {
      plugin.mount(slot, rec.id);
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

  // Initial load
  void (async () => {
    try {
      const list = await invoke<WidgetRecord[]>("floaty_list");
      const mon = await monitorArea();
      resolveOverlayLayout(list, mon);
      for (const rec of list) {
        mountWidget(rec);
      }
      scheduleHitRectsUpdate();
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
        const { w, h } = getInitialWidgetSize(rec);
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

  // Global escape key to collapse open folders / close menus
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      document.querySelectorAll(".pin-menu, .model-menu").forEach((m) => m.remove());
      document.querySelectorAll(".folder.open .fshut").forEach((b) => (b as HTMLElement).click());
      scheduleHitRectsUpdate();
    }
  });
}
