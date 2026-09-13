/**
 * What the backend knows about each widget kind — the mirror of
 * `src-tauri/src/plugins.rs`, fetched once per window through `floaty_plugins`.
 *
 * This module is deliberately a leaf: it imports nothing from the widget modules
 * (only Tauri and a type), so core code such as `lib.ts` can ask "is this floatie
 * a file on disk?" and "how big is it?" without pulling in the plugin registry —
 * and without hardcoding kind names of its own.
 */

import { invoke } from "@tauri-apps/api/core";
import type { WidgetRecord } from "./lib";

/** Keep this module import-free apart from the type above: `lib.ts` imports it
 *  back, and a runtime cycle would be resolved in whichever order the bundler
 *  happens to pick. */
function log(msg: string): void {
  void invoke("floaty_log", { msg }).catch(() => undefined);
}

/** One plugin, as the Rust manifest describes it. */
export interface PluginManifestEntry {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  /** Fresh-widget size, `[w, h]`. */
  default_size: [number, number];
  resizable: boolean;
  /** Set for kinds that stand for something on disk. */
  desktop_item: PluginDesktopItem | null;
  /** Quick-add button label, or null for "no button in + New floatie". */
  add_label: string | null;
  /** Overlay placement order, lower first. */
  layout_priority: number;
  /** "builtin" or "installed". */
  source: "builtin" | "installed";
  /** Absolute path of the module to import (installed plugins only). */
  entry: string | null;
  version: string;
  author: string;
}

export interface PluginDesktopItem {
  path_key: string;
  noun: string;
  group: boolean;
}

export interface Size {
  w: number;
  h: number;
}

/** Size for a record whose kind this build does not know. */
const FALLBACK_SIZE: Size = { w: 100, h: 100 };

const entries = new Map<string, PluginManifestEntry>();
let pending: Promise<void> | undefined;

function num(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/**
 * Fetch and cache the manifest. Idempotent and safe to call from every window:
 * the first call does the round trip, later ones join it.
 */
export function loadPluginManifest(): Promise<void> {
  pending ??= invoke<PluginManifestEntry[]>("floaty_plugins")
    .then((list) => {
      for (const entry of list) entries.set(entry.id, entry);
    })
    .catch((err: unknown) => {
      // A missing manifest must not take the overlay down; the fallbacks below
      // keep the layout sane and the log says why the sizes look generic.
      log(`[plugin] manifest load failed: ${String(err)}`);
    });
  return pending;
}

export function manifestFor(kind: string): PluginManifestEntry | undefined {
  return entries.get(kind);
}

/** The plugins the user installed, in manifest order. */
export function installedEntries(): PluginManifestEntry[] {
  return Array.from(entries.values()).filter((e) => e.source === "installed" && e.entry);
}

/** Overlay placement order for a kind; unknown kinds sink to the icon grid. */
export function layoutPriorityFor(kind: string): number {
  return entries.get(kind)?.layout_priority ?? DEFAULT_LAYOUT_PRIORITY;
}

/** Priority of the icon/folder grid: anything hand-placed sorts before it. */
export const DEFAULT_LAYOUT_PRIORITY = 10;

/** Every plugin the backend has, in manifest order. */
export function manifestEntries(): PluginManifestEntry[] {
  return Array.from(entries.values());
}

/**
 * The desktop-item capability: file, folder and shortcut floaties carry a real
 * path, get the shell verbs and are deleted through the Recycle Bin.
 */
export function desktopItemFor(kind: string): PluginDesktopItem | undefined {
  return entries.get(kind)?.desktop_item ?? undefined;
}

/** True when a floatie of this kind stands for something on disk. */
export function isDesktopItem(kind: string): boolean {
  return desktopItemFor(kind) !== undefined;
}

/**
 * The on-disk path a floatie points at. Desktop items keep it under the key the
 * manifest names ("target" for one item, "path" for a folder); until the
 * manifest has arrived we still find it by looking at both keys, so a menu that
 * opens in the first milliseconds behaves.
 */
export function pathOf(rec: WidgetRecord): string {
  const key = desktopItemFor(rec.kind)?.path_key;
  const value = key ? rec.data[key] : rec.data["target"] ?? rec.data["path"];
  return typeof value === "string" ? value : "";
}

/**
 * The size a record occupies on screen: the record's own w/h when the widget is
 * resizable, otherwise the manifest's default for the kind.
 */
export function pluginSize(rec: WidgetRecord): Size {
  const advertised = entries.get(rec.kind)?.default_size;
  const base: Size =
    Array.isArray(advertised) && advertised.length === 2 && advertised.every((n) => num(n) !== undefined)
      ? { w: advertised[0], h: advertised[1] }
      : FALLBACK_SIZE;
  return {
    w: num(rec.data["w"]) ?? base.w,
    h: num(rec.data["h"]) ?? base.h,
  };
}

/**
 * Cross-check the built-in plugins at runtime: a kind the backend serves but no
 * module mounts (or the other way round) is a build mistake — the check script
 * catches it in CI, this names the window it happened in. Installed plugins are
 * reported by the loader instead, since their module is imported at runtime.
 */
export function crossCheckPlugins(kinds: string[]): { missingModule: string[]; missingManifest: string[] } {
  const known = new Set<string>();
  const missingModule: string[] = [];
  for (const entry of entries.values()) {
    known.add(entry.id);
    if (!kinds.includes(entry.id)) missingModule.push(entry.id);
  }
  const missingManifest = kinds.filter((kind) => !known.has(kind) && entries.size > 0);
  if (missingModule.length || missingManifest.length) {
    log(
      `[plugin] registry mismatch — backend-only: [${missingModule.join(", ")}] frontend-only: [${missingManifest.join(", ")}]`,
    );
  }
  return { missingModule, missingManifest };
}
