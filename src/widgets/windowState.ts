import { getCurrentWindow } from "@tauri-apps/api/window";

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

/** A layer label, with the optional `-<screen>` suffix. */
function isLayerLabel(label: string, base: string): boolean {
  return label === base || label.startsWith(`${base}-`);
}

/** Whether this is an always-on-top overlay, on any screen. */
export function isTopOverlayLabel(label: string): boolean {
  return isLayerLabel(label, TOP_LAYER);
}

export function isOverlayMode(): boolean {
  return (
    isLayerLabel(appWin.label, DESKTOP_LAYER) ||
    isTopOverlayLabel(appWin.label) ||
    window.location.hash.startsWith("#/overlay")
  );
}

/**
 * Whether this page is the always-on-top layer, the one that draws the widgets
 * pinned above other windows. Both layers are full-screen overlays running the
 * same mount code; they differ in which records they draw.
 */
export function isTopLayer(): boolean {
  return isTopOverlayLabel(appWin.label);
}
