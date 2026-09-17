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
import { listen } from "@tauri-apps/api/event";
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
  /**
   * Move the widget (handles the overlay slot and the hit rects). Pass
   * `{transient: true}` when the slot is only being stretched to catch the mouse —
   * a drawing surface, a page of slots — so the position watcher does not save that
   * scaffolding as the widget's place. Any later `setPos` without it clears the flag.
   */
  setPos: (id: string, x: number, y: number, opts?: { transient?: boolean }) => void;
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
  /** Current global settings. */
  settings: () => FloatSettings;
  /**
   * Re-run `cb` whenever the settings change, and once as soon as they are
   * loaded. This is how a widget follows the settings: it is the same path the
   * built-ins use, and it runs after floaty has updated its own cache.
   *
   * Do *not* register your own `listen("floaty-settings-changed")` instead:
   * floaty's cache updater and your handler are then two listeners on one event,
   * and yours can run with the values from before the change.
   */
  onSettings: (cb: () => void) => void;
  /** Load the settings once if nothing has yet. Not needed before `settings()`
   *  — that reads whatever is cached — and it tells you nothing about changes. */
  watchSettings: () => void;
  /** The caption rule the desktop uses for a name: `Arc.lnk` reads as `Arc`.
   *  Use it wherever your widget labels a file, so it matches its neighbours. */
  displayName: (name: string) => string;
  /** The desktop area this widget may live in. */
  monitorArea: () => Promise<MonitorArea>;

  // ---- apiVersion 2 ----
  // Additive. A manifest that says `"apiVersion": 1` gets the surface above and
  // nothing here: a plugin cannot come to depend on a member its own manifest says
  // it does not have.

  /** The api contract this build speaks. */
  apiVersion: number;
  /**
   * Say something in a Windows notification. Raised in Rust, so it costs you no
   * permission of your own — and it is the only way out of a widget the user will
   * notice while the desktop is covered by something else.
   */
  notify: (title: string, body?: string) => Promise<void>;
  /**
   * Run `fn` every `ms` while the widget is mounted. The timer belongs to the
   * widget: it is cleared when the widget is removed or remounted, so a plugin
   * cannot outlive its own widget by leaking one.
   */
  every: (id: string, ms: number, fn: () => void) => void;
  /** Run `fn` once, `ms` from now, unless the widget goes away first. */
  after: (id: string, ms: number, fn: () => void) => void;
  /**
   * Follow one of floaty's own events. Cleaned up with the widget, and returns a
   * function that stops listening early. See `PluginEventName` for the list.
   */
  on: (id: string, event: PluginEventName, fn: (payload: unknown) => void) => () => void;
}

/** The events a plugin may listen for, and the app event each is forwarded from. */
export type PluginEventName =
  | "settings"
  | "widget-updated"
  | "widget-removed"
  | "plugins-changed"
  | "display";

const FORWARDED: Array<[PluginEventName, string]> = [
  ["settings", "floaty-settings-changed"],
  ["widget-updated", "floaty-widget-updated"],
  ["widget-removed", "floaty-widget-removed"],
  ["plugins-changed", "floaty-plugins-changed"],
  ["display", "floaty-display-changed"],
];

/** What a widget owns: its timers, and who is listening to what. */
interface Scope {
  timers: Set<number>;
  listeners: Map<PluginEventName, Set<(payload: unknown) => void>>;
}

const scopes = new Map<string, Scope>();

function scopeOf(id: string): Scope {
  let scope = scopes.get(id);
  if (!scope) {
    scope = { timers: new Set(), listeners: new Map() };
    scopes.set(id, scope);
  }
  return scope;
}

/**
 * Drop everything a widget owns: its timers and its listeners.
 *
 * Called when the widget goes away — removed, remounted, or its kind switched off
 * — because a timer a plugin started is otherwise a widget that never dies: it
 * keeps running after the thing that wanted it is gone, which is the shape of
 * every slow leak in a long-running desktop.
 */
export function clearWidgetScope(id: string): void {
  const scope = scopes.get(id);
  if (!scope) return;
  for (const timer of scope.timers) window.clearInterval(timer);
  scopes.delete(id);
}

/** One forwarder per event name per window, not one per widget. */
const forwarding = new Set<PluginEventName>();

function ensureForwarded(name: PluginEventName): void {
  if (forwarding.has(name)) return;
  forwarding.add(name);
  const source = FORWARDED.find(([pluginName]) => pluginName === name)?.[1];
  if (!source) return;
  void listen(source, (event) => {
    for (const scope of scopes.values()) {
      const handlers = scope.listeners.get(name);
      if (!handlers) continue;
      for (const handler of handlers) {
        try {
          handler(event.payload);
        } catch (err) {
          widgetApi.log(`[plugin] a "${name}" listener threw: ${String(err)}`);
        }
      }
    }
  }).catch((e: unknown) => widgetApi.log(`[plugin] could not follow "${name}": ${String(e)}`));
}

/** What a v1 plugin gets instead of the new members: a sentence, not silence. */
const notInV1 = (what: string) => () => {
  throw new Error(`${what} needs "apiVersion": 2 in plugin.json`);
};

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
  /**
   * A fresh row bound to a shared setting — `gravity`, `pet_speed`, one of the
   * names in `settings/params.ts` — to drop into the plugin's card. Returns
   * `undefined` for a name this build does not have, so a plugin asking for a
   * setting that has been renamed draws nothing instead of an empty row.
   *
   * Rows that apply to every desktop item (motion, click behaviour) also have a
   * home in the Motion tab; ask for one here only when the plugin's own card is
   * the better place for it.
   */
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
  /** Optional custom rows or controls appended to the plugin's card in the Plugins tab */
  renderSettings?: (card: HTMLElement, ctx: PluginSettingsContext) => void | Promise<void>;
}

/**
 * The plugin contract this build hands out. Must match `PLUGIN_API_VERSION` in
 * `src-tauri/src/plugins.rs` — `scripts/check-plugins.mjs` reads both and fails
 * when they drift.
 */
export const WIDGET_API_VERSION = 2;

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
  setPos: (id, x, y, opts) => floaty.setWidgetPos(id, x, y, 1, opts),
  setSize: floaty.setWidgetSize,
  addPinMenu: floaty.addPinMenu,
  addResizeHandle: floaty.addResizeHandle,
  removeSelf: floaty.removeSelf,
  settings: floaty.currentSettings,
  onSettings: floaty.onSettings,
  watchSettings: floaty.watchSettings,
  displayName: floaty.displayName,
  monitorArea: floaty.monitorArea,
  apiVersion: WIDGET_API_VERSION,
  notify: async (title: string, body = "") => {
    await invoke("floaty_notify", { title, body });
  },
  every: (id, ms, fn) => {
    const scope = scopeOf(id);
    scope.timers.add(window.setInterval(fn, ms));
  },
  after: (id, ms, fn) => {
    const scope = scopeOf(id);
    const timer = window.setTimeout(() => {
      scope.timers.delete(timer);
      fn();
    }, ms);
    scope.timers.add(timer);
  },
  on: (id, event, fn) => {
    const scope = scopeOf(id);
    let handlers = scope.listeners.get(event);
    if (!handlers) {
      handlers = new Set();
      scope.listeners.set(event, handlers);
    }
    handlers.add(fn);
    ensureForwarded(event);
    return () => {
      handlers?.delete(fn);
    };
  },
};

/**
 * The surface a manifest that says `"apiVersion": 1` gets.
 *
 * The new members are there but refuse: a plugin that calls one is told what to
 * put in its manifest, which is more use than "api.every is not a function".
 */
const legacyApi: WidgetApi = {
  ...widgetApi,
  apiVersion: 1,
  notify: refuse("notify"),
  every: refuse("every"),
  after: refuse("after"),
  on: refuse("on"),
};

function refuse(what: string): (...args: unknown[]) => never {
  return () => {
    throw new Error(`api.${what} needs "apiVersion": 2 in plugin.json`);
  };
}

/** The api to hand a plugin of this kind: the surface its manifest promised. */
export function pluginApi(kind: string): WidgetApi {
  const entry = installedEntries().find((candidate) => candidate.id === kind);
  if (entry && entry.api_version < WIDGET_API_VERSION) return legacyApi;
  return widgetApi;
}

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
      // Unapproved code is not imported at all. A plugin is approved when it is
      // installed from an archive (that dialog shows what is in it, who wrote it
      // and what it replaces) or from the Plugins tab afterwards; a folder that
      // appeared by some other route is not, and neither is one whose files changed
      // since it was approved.
      if (!entry.trusted) {
        widgetApi.log(`[plugin] ${entry.id} is not approved — approve it in the Plugins tab`);
        continue;
      }
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
