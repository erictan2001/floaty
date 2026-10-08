import { invoke } from "@tauri-apps/api/core";
import { appWin, isOverlayMode } from "./windowState";

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

  // Anything the page draws that has to catch the mouse needs a rect here, because
  // the overlay window is click-through everywhere else. The first-run guard's card
  // is one of them: it is the whole point of the screen that the Confirm button can
  // be pressed (ADR 0004) — and the veil behind it is *not* listed, so the desktop
  // it covers still takes the clicks it always did.
  const popups = document.querySelectorAll(
    ".pin-menu, .model-menu, .live2d-palette-modal, .live2d-palette, .fname-edit, .first-run-guard",
  );
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
export function isOverlayDragging(): boolean {
  return overlayDragging;
}

export function notifyDragging(dragging: boolean): void {
  overlayDragging = dragging;
  if (isOverlayMode()) {
    if (!dragging) {
      syncHitRectsToRust();
    }
    // per overlay layer: the drag only clears the region of the window being
    // dragged in, or the other layer loses its clicks for the duration
    invoke("floaty_set_overlay_dragging", { label: appWin.label, dragging }).catch(() => undefined);
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
