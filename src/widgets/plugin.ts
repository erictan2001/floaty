/**
 * The widget plugin contract.
 *
 * A plugin module owns *behaviour*: how to mount its widget, how to describe it
 * in the settings list, which rows it draws there. Everything else about a kind
 * — its display name, its size, its quick-add label, whether it is a desktop
 * item — comes from the backend manifest (`pluginManifest.ts`, backed by
 * `src-tauri/src/plugins.rs`), so those facts exist in exactly one place and
 * work the same for a plugin the user installed as for a built-in.
 *
 * Three ways a plugin gets here:
 *
 * 1. a built-in module in this folder, listed in `BUILTINS` below;
 * 2. a plugin the user installed, imported at runtime from
 *    `<app data>/plugins/<id>/index.js` (see `loadPlugins`);
 * 3. `registerPlugin()` by hand, for something that is neither.
 *
 * To add a built-in plugin: write the module, add it to `BUILTINS`, add the
 * matching entry to the Rust manifest. `npm run build` runs
 * `scripts/check-plugins.mjs`, which fails when the two id lists disagree.
 */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import * as floaty from "./lib";
import type { FloatSettings, MonitorArea, PinMenuOptions, WidgetRecord } from "./lib";
import { installedEntries, loadPluginManifest } from "./pluginManifest";
import { notePlugin } from "./note";
import { clockPlugin } from "./clock";
import { petPlugin } from "./pet";
import { appPlugin, filePlugin } from "./appicon";
import { folderPlugin } from "./folder";
import { live2dPlugin } from "./live2dPlugin";
import { visualizerPlugin } from "./visualizer";
import { sysmonPlugin } from "./sysmon";

export interface PluginRecord {
  id: string;
  kind?: string;
  x?: number;
  y?: number;
  data: Record<string, unknown>;
}

/**
 * What a plugin is handed at mount time. This is the supported surface: a
 * third-party plugin uses it instead of importing floaty's internals, so the
 * internals stay free to move.
 */
export interface WidgetApi {
  /** Call any backend command (`floaty_list`, `floaty_save`, ...). */
  invoke: typeof invoke;
  /** Append a line to floaty.log — the only way to debug inside the overlay. */
  log: (msg: string) => void;
  record: {
    load: (id: string) => Promise<WidgetRecord | undefined>;
    save: (rec: WidgetRecord) => Promise<void>;
  };
  /**
   * Make an element drag the widget around — the whole surface, or a title bar.
   * A press only counts as a drag after a few px of travel, so a widget that
   * also reacts to clicks (a tap to edit) still gets its click, and controls
   * inside the element (button, input, the resize grip) keep their behaviour.
   *
   * Pass the record: this also keeps the moved position saved. `enableDrag`
   * without that was the half-wired version — the widget followed the cursor and
   * then snapped back on the next launch.
   */
  enableDrag: (el: HTMLElement, rec: WidgetRecord, opts?: { threshold?: number }) => void;
  /** Move the widget (handles the overlay slot and the hit rects). */
  setPos: (id: string, x: number, y: number) => void;
  /** Resize the widget slot. */
  setSize: (id: string, w: number, h: number) => void;
  /** Right-click menu with the standard floaty rows for this widget. Pass
   *  `options.rows` to add the plugin's own rows (see `PinMenuApi`). */
  addPinMenu: (
    wrap: HTMLElement,
    getRec: () => WidgetRecord | undefined,
    options?: PinMenuOptions,
  ) => void;
  /** Bottom-right drag grip that resizes and persists the record. */
  addResizeHandle: (wrap: HTMLElement, rec: WidgetRecord, minW: number, minH: number) => void;
  /** Remove the widget (asks first when the user enabled confirmation). */
  removeSelf: (rec: WidgetRecord) => Promise<void>;
  /** Current global settings, and a hook to react when they change. */
  settings: () => FloatSettings;
  watchSettings: () => void;
  /** The desktop area this widget may live in. */
  monitorArea: () => Promise<MonitorArea>;
}

export interface WidgetControlContext {
  /** Safely run an asynchronous backend invocation with error reporting */
  safe: <T>(label: string, fn: () => Promise<T>) => Promise<T | undefined>;
  /** Request refreshing the widget list on the settings page */
  refreshWidgets: () => Promise<void>;
  /** Show an error banner in settings */
  showError: (msg: string) => void;
}

export interface PluginSettingsContext {
  /** Get current global floating settings */
  getSettings: () => FloatSettings;
  /** Retrieve a shared parameter row element (e.g. 'pet_speed', 'gravity', etc.) */
  getSharedRow: (name: string) => HTMLElement | undefined;
  /** Safely run an asynchronous backend invocation with error reporting */
  safe: <T>(label: string, fn: () => Promise<T>) => Promise<T | undefined>;
  /** Show an error banner in settings */
  showError: (msg: string) => void;
  /** Request refreshing the widget list on the settings page */
  refreshWidgets: () => Promise<void>;
  /** Save modified global settings */
  updateSettings: (patch: Partial<FloatSettings>) => Promise<void>;
}

export interface FloatyPlugin {
  /** Unique plugin identifier, matching the id in the Rust manifest */
  readonly kind: string;
  /**
   * Mount the widget DOM into the container for the given record ID. `api` is
   * the supported surface for talking to floaty; built-ins may also import
   * `./lib` directly, third-party plugins should not.
   */
  mount: (root: HTMLElement, id: string, api: WidgetApi) => void | Promise<void>;
  /** Detail summary shown in the "On your desktop" list (or undefined for just the ID) */
  describe?: (rec: PluginRecord) => string | undefined;
  /** Optional custom controls to insert into the widget's card in "On your desktop" */
  renderWidgetControls?: (
    card: HTMLElement,
    rec: PluginRecord,
    ctx: WidgetControlContext,
    deleteBtn: HTMLElement,
  ) => void | Promise<void>;
  /** Optional custom rows or controls appended to the plugin's card in the "Plugins" list */
  renderSettings?: (card: HTMLElement, ctx: PluginSettingsContext) => void | Promise<void>;
}

/** The api object handed to every `mount`. */
export const widgetApi: WidgetApi = {
  invoke,
  log: (msg: string) => {
    void invoke("floaty_log", { msg }).catch(() => undefined);
  },
  record: { load: floaty.loadRecord, save: floaty.saveRecord },
  enableDrag: (el, rec, opts) => {
    floaty.enableOverlayDrag(el, rec.id, opts);
    floaty.trackPosition(rec);
  },
  setPos: floaty.setWidgetPos,
  setSize: floaty.setWidgetSize,
  addPinMenu: floaty.addPinMenu,
  addResizeHandle: floaty.addResizeHandle,
  removeSelf: floaty.removeSelf,
  settings: floaty.currentSettings,
  watchSettings: floaty.watchSettings,
  monitorArea: floaty.monitorArea,
};

/**
 * The built-in plugins, in the order the settings window lists them. This is the
 * single list — the registry is derived from it.
 */
const BUILTINS: FloatyPlugin[] = [
  notePlugin,
  clockPlugin,
  petPlugin,
  appPlugin,
  filePlugin,
  folderPlugin,
  live2dPlugin,
  visualizerPlugin,
  sysmonPlugin,
];

const registry = new Map<string, FloatyPlugin>();

for (const plugin of BUILTINS) {
  register(plugin);
}

function register(plugin: FloatyPlugin): void {
  if (!plugin.kind || !plugin.mount) {
    throw new Error(`plugin registration: ${JSON.stringify(plugin)} needs a kind and a mount`);
  }
  if (registry.has(plugin.kind)) {
    throw new Error(`plugin registration: duplicate kind "${plugin.kind}"`);
  }
  registry.set(plugin.kind, plugin);
}

/** Register a plugin that is not in `BUILTINS` (used by the loader). */
export function registerPlugin(plugin: FloatyPlugin): void {
  register(plugin);
}

export function pluginFor(kind: string): FloatyPlugin | undefined {
  return registry.get(kind);
}

export function allPlugins(): readonly FloatyPlugin[] {
  return Array.from(registry.values());
}

/** Kinds the frontend can mount, for the manifest cross-check. */
export function pluginKinds(): string[] {
  return Array.from(registry.keys());
}

let loaded: Promise<void> | undefined;

/**
 * Fetch the plugin manifest and import every installed plugin's module.
 *
 * Runs once per window (the promise is shared); call it before mounting
 * anything, since a widget whose kind comes from an installed plugin has no
 * module until this resolves. A plugin that fails to import is logged and
 * skipped — a broken plugin must not keep the desktop from coming up.
 */
export function loadPlugins(): Promise<void> {
  loaded ??= (async () => {
    await loadPluginManifest();
    for (const entry of installedEntries()) {
      if (registry.has(entry.id)) continue;
      try {
        const module = (await import(/* @vite-ignore */ convertFileSrc(entry.entry!))) as {
          default?: Partial<FloatyPlugin>;
        };
        const plugin = module.default;
        if (!plugin || typeof plugin.mount !== "function") {
          throw new Error("the module's default export needs a mount() function");
        }
        register({ ...plugin, kind: entry.id } as FloatyPlugin);
        widgetApi.log(`[plugin] ${entry.id} loaded (${entry.name} ${entry.version})`);
      } catch (err) {
        widgetApi.log(`[plugin] ${entry.id} failed to load: ${String(err)}`);
      }
    }
  })();
  return loaded;
}
