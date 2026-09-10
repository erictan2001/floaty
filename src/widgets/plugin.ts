import { mountNote } from "./note";
import { mountClock } from "./clock";
import { mountPet } from "./pet";
import { mountLauncher } from "./appicon";
import { mountFolder } from "./folder";

export interface PluginRecord {
  id: string;
  data: Record<string, unknown>;
}

export interface FloatyPlugin {
  kind: string;
  name: string;
  /** quick-add button label; absent when the plugin isn't directly creatable */
  addLabel?: string;
  mount: (root: HTMLElement, id: string) => void;
  /** short detail for the settings widget list, or undefined for just the id */
  describe: (rec: PluginRecord) => string | undefined;
}

function textPreview(rec: PluginRecord, max: number): string | undefined {
  const t = rec.data["text"];
  return typeof t === "string" && t ? t.slice(0, max) : undefined;
}

function namedPreview(rec: PluginRecord): string | undefined {
  const n = rec.data["name"];
  return typeof n === "string" && n ? n : undefined;
}

export const plugins: FloatyPlugin[] = [
  {
    kind: "note",
    name: "Note",
    addLabel: "+ note",
    mount: mountNote,
    describe: (r) => textPreview(r, 32),
  },
  {
    kind: "clock",
    name: "Clock",
    addLabel: "+ clock",
    mount: mountClock,
    describe: () => undefined,
  },
  {
    kind: "pet",
    name: "Pet",
    addLabel: "+ pet",
    mount: mountPet,
    describe: namedPreview,
  },
  {
    kind: "app",
    name: "App launcher",
    mount: mountLauncher,
    describe: namedPreview,
  },
  {
    kind: "folder",
    name: "Folder",
    addLabel: "+ folder",
    mount: mountFolder,
    describe: (r) => {
      const raw = r.data["items"];
      const n = Array.isArray(raw) ? raw.length : 0;
      const nm = namedPreview(r) ?? "folder";
      return `${nm} (${n} app${n === 1 ? "" : "s"})`;
    },
  },
  {
    kind: "live2d",
    name: "Live2D",
    addLabel: "+ live2d",
    // lazy: keeps ~600KB of pixi out of every other widget window
    mount: (root, id) => {
      void import("./live2d").then((m) => m.mountLive2D(root, id));
    },
    describe: namedPreview,
  },
];

export function pluginFor(kind: string): FloatyPlugin | undefined {
  return plugins.find((p) => p.kind === kind);
}
