/**
 * The settings window's state: the settings themselves, the floaties on the
 * desktop, and the plugin list.
 *
 * Panes *read* from here and load nothing themselves, which is what keeps a
 * re-render free — and why a pane never fights the data it is drawing. Anything
 * that changes the data calls a reload (or `updateSettings`), and every
 * subscriber hears about it through `changes()`.
 */

import { invoke } from "@tauri-apps/api/core";
import { DEFAULT_SETTINGS, type FloatSettings, type WidgetRecord } from "../widgets/lib";
import type { PluginManifestEntry } from "../widgets/pluginManifest";
import { safe } from "./api";

/**
 * How long a settings edit waits before it is written.
 *
 * A slider drag fires `input` on every step; without this the whole settings
 * file would be written (and every widget told) once per pixel.
 */
const SAVE_DEBOUNCE_MS = 250;

let settingsState: FloatSettings = { ...DEFAULT_SETTINGS };
let widgetState: WidgetRecord[] = [];
let pluginState: PluginManifestEntry[] = [];
const listeners = new Set<() => void>();

export function settings(): FloatSettings {
  return settingsState;
}

export function widgets(): readonly WidgetRecord[] {
  return widgetState;
}

export function plugins(): readonly PluginManifestEntry[] {
  return pluginState;
}

/** Kinds the user turned off: no quick-add button, no row in the list. */
export function disabledKinds(): Set<string> {
  return new Set(pluginState.filter((p) => !p.enabled).map((p) => p.id));
}

/** Subscribe to data changes; returns the unsubscribe. */
export function changes(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function notify(): void {
  for (const fn of listeners) fn();
}

export async function loadSettings(): Promise<void> {
  const loaded = await safe("load settings", () => invoke<FloatSettings>("floaty_get_settings"));
  if (!loaded) return;
  settingsState = { ...DEFAULT_SETTINGS, ...loaded };
  notify();
}

export async function reloadWidgets(): Promise<void> {
  const list = await safe("load floaties", () => invoke<WidgetRecord[]>("floaty_list"));
  widgetState = list ?? [];
  notify();
}

export async function reloadPlugins(): Promise<void> {
  const list = await safe("load plugins", () => invoke<PluginManifestEntry[]>("floaty_plugins"));
  if (!list) return;
  pluginState = list;
  notify();
}

/**
 * Patch the cached settings and write them.
 *
 * Debounced by default (a slider drag is one write); pass `now` when the next
 * step depends on the backend already having them — browsing to a folder and
 * then asking the backend to scan it, for instance.
 */
export function updateSettings(patch: Partial<FloatSettings>, opts: { now?: boolean } = {}): Promise<void> {
  settingsState = { ...settingsState, ...patch };
  if (opts.now) {
    window.clearTimeout(saveTimer);
    return flushSettings();
  }
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => void flushSettings(), SAVE_DEBOUNCE_MS);
  return Promise.resolve();
}

let saveTimer: number | undefined;

async function flushSettings(): Promise<void> {
  const sent = { ...settingsState };
  const stored = await safe("save settings", () =>
    invoke<FloatSettings>("floaty_set_settings", { settings: sent }),
  );
  // The backend clamps what it stores — and refuses a start-on-boot switch it
  // could not write — so take its answer as the truth and redraw if it differs
  // from what was sent. Otherwise the window keeps showing the value the user
  // asked for while the app runs with the one it actually accepted.
  if (stored) {
    const differs = (Object.keys(stored) as (keyof FloatSettings)[]).some(
      (key) => sent[key] !== stored[key],
    );
    settingsState = { ...settingsState, ...stored };
    if (differs) notify();
  }
}

export async function setPluginEnabled(id: string, enabled: boolean): Promise<void> {
  await safe("toggle plugin", () => invoke("floaty_set_plugin_enabled", { id, enabled }));
  await reloadPlugins();
}

/** Create a floatie of this kind and return its record. */
export async function addFloatie(kind: string): Promise<WidgetRecord | undefined> {
  return safe("add floatie", () => invoke<WidgetRecord>("floaty_create", { kind }));
}

/** Float an app by its shortcut path (the Apps tab). */
export async function addLauncher(name: string, path: string): Promise<WidgetRecord | undefined> {
  return safe("float app", () => invoke<WidgetRecord>("floaty_add_launcher", { name, path }));
}

/** Reload what the backend says about plugin folders, returning its rejections. */
export async function rescanPlugins(): Promise<string[] | undefined> {
  return safe("rescan plugins", () => invoke<string[]>("floaty_rescan_plugins"));
}
