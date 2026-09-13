import type { FloatSettings } from "./lib";
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
  /** Unique plugin identifier (e.g. "note", "clock", "pet", "live2d") */
  readonly kind: string;
  /** Human-readable display name shown in settings */
  readonly name: string;
  /** Quick-add button label (e.g. "+ note"). If omitted, no button is shown in "+ New floatie" */
  readonly addLabel?: string;
  /** Mount the widget DOM into the container for the given record ID */
  mount: (root: HTMLElement, id: string) => void | Promise<void>;
  /** Detail summary shown in the "On your desktop" list (or undefined for just the ID) */
  describe: (rec: PluginRecord) => string | undefined;
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

const registry = new Map<string, FloatyPlugin>();

export function registerPlugin(plugin: FloatyPlugin): void {
  registry.set(plugin.kind, plugin);
}

export function pluginFor(kind: string): FloatyPlugin | undefined {
  return registry.get(kind);
}

export function allPlugins(): FloatyPlugin[] {
  return Array.from(registry.values());
}

// Built-in plugin registrations
registerPlugin(notePlugin);
registerPlugin(clockPlugin);
registerPlugin(petPlugin);
registerPlugin(appPlugin);
registerPlugin(filePlugin);
registerPlugin(folderPlugin);
registerPlugin(live2dPlugin);
registerPlugin(visualizerPlugin);
registerPlugin(sysmonPlugin);

/** Exported array for backward compatibility and simple iteration */
export const plugins: FloatyPlugin[] = [
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
