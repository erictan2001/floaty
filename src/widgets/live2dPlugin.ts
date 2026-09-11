import { invoke } from "@tauri-apps/api/core";
import { ensureLive2DCore } from "./lib";
import type { FloatyPlugin, PluginRecord, PluginSettingsContext, WidgetControlContext } from "./plugin";

export interface Live2dModelEntry {
  name: string;
  path: string;
}

let cachedModels: Live2dModelEntry[] = [];
let scanListeners: Array<() => void> = [];

export function getCachedModels(): Live2dModelEntry[] {
  return cachedModels;
}

export function onModelsChanged(fn: () => void): () => void {
  scanListeners.push(fn);
  return () => {
    scanListeners = scanListeners.filter((l) => l !== fn);
  };
}

export async function scanModels(root: string): Promise<Live2dModelEntry[]> {
  if (!root) {
    cachedModels = [];
    scanListeners.forEach((l) => l());
    return [];
  }
  try {
    const found = await invoke<Live2dModelEntry[]>("floaty_scan_models", { root });
    cachedModels = found || [];
  } catch {
    cachedModels = [];
  }
  scanListeners.forEach((l) => l());
  return cachedModels;
}

export const live2dPlugin: FloatyPlugin = {
  kind: "live2d",
  name: "Live2D",
  addLabel: "+ live2d",

  // lazy: keeps pixi out of every other widget window. Core MUST load
  // before the import — cubism2 throws at module evaluation without it.
  mount: (root, id) => {
    void (async () => {
      try {
        await ensureLive2DCore();
        const m = await import("./live2d");
        m.mountLive2D(root, id);
      } catch (e) {
        invoke("floaty_log", { msg: `[webview live2d/${id}] mount failed: ${String(e)}` }).catch(
          () => undefined,
        );
        const f = document.createElement("div");
        f.className = "live2d-failed";
        f.textContent = "live2d failed to load";
        root.append(f);
      }
    })();
  },

  describe: (rec: PluginRecord) => {
    const n = rec.data["name"];
    return typeof n === "string" && n ? n : undefined;
  },

  renderWidgetControls: (card, rec, ctx, deleteBtn) => {
    const sel = document.createElement("select");
    sel.className = "select mini";
    const cur = typeof rec.data["model"] === "string" ? (rec.data["model"] as string) : "";

    const populate = () => {
      sel.innerHTML = "";
      const seenValues = new Set<string>();
      const seenLabels = new Set<string>();
      const addOpt = (label: string, value: string): void => {
        if (seenValues.has(value)) return;
        seenValues.add(value);
        let finalLabel = label;
        if (seenLabels.has(finalLabel)) {
          const parent = value.split(/[/\\]/).slice(-2, -1)[0];
          if (parent) finalLabel = `${label} (${parent})`;
        }
        seenLabels.add(finalLabel);
        const opt = document.createElement("option");
        opt.value = value;
        opt.textContent = finalLabel;
        sel.append(opt);
      };

      if (!cur) {
        addOpt(cachedModels.length > 0 ? "Select a model..." : "No models found (set folder above)", "");
      }
      for (const m of cachedModels) addOpt(m.name, m.path);
      if (cur && !seenValues.has(cur)) {
        addOpt(`${cur.split(/[/\\]/).pop() ?? cur} (current)`, cur);
      }
      sel.value = cur;
      sel.title = sel.options[sel.selectedIndex]?.text || "";
    };

    populate();
    const unsub = onModelsChanged(() => {
      if (!sel.isConnected && card.isConnected === false) {
        unsub();
        return;
      }
      populate();
    });

    sel.addEventListener("change", () => {
      sel.title = sel.options[sel.selectedIndex]?.text || "";
      void ctx.safe("set model", () =>
        invoke("floaty_set_widget_model", { id: rec.id, model: sel.value }),
      );
    });

    if (deleteBtn && deleteBtn.parentNode === card) {
      card.insertBefore(sel, deleteBtn);
    } else {
      card.append(sel);
    }
  },

  renderSettings: (card, ctx) => {
    const currentSettings = ctx.getSettings();
    let currentRoot = currentSettings.live2d_root || "";

    const row = document.createElement("div");
    row.className = "slider-row";

    const label = document.createElement("span");
    label.className = "slider-label";
    label.textContent = "models";

    const path = document.createElement("span");
    path.className = "folder-path";
    path.id = "live2d-root";
    const updatePathLabel = () => {
      path.textContent = currentRoot || "no folder set";
      path.title = currentRoot;
    };
    updatePathLabel();

    const count = document.createElement("div");
    count.className = "count";
    count.id = "live2d-count";
    const updateCount = () => {
      count.textContent =
        cachedModels.length === 0
          ? "no models scanned yet"
          : `${cachedModels.length} model${cachedModels.length === 1 ? "" : "s"} found`;
    };
    updateCount();

    const runScan = async () => {
      await ctx.safe("scan models", () => scanModels(currentRoot));
      updateCount();
      await ctx.refreshWidgets();
    };

    const browse = document.createElement("button");
    browse.className = "pill small";
    browse.textContent = "browse";
    browse.addEventListener("click", async () => {
      if (browse.disabled) return;
      browse.disabled = true;
      try {
        const { open } = await import("@tauri-apps/plugin-dialog");
        const picked = await open({ directory: true, multiple: false });
        if (typeof picked === "string" && picked) {
          currentRoot = picked;
          updatePathLabel();
          await ctx.updateSettings({ live2d_root: picked });
          await runScan();
        }
      } catch {
        ctx.showError("folder picker failed");
      } finally {
        browse.disabled = false;
      }
    });

    const scanBtn = document.createElement("button");
    scanBtn.className = "pill small ghost";
    scanBtn.textContent = "scan";
    scanBtn.addEventListener("click", () => void runScan());

    row.append(label, path, browse, scanBtn);
    card.append(row, count);

    // Initial scan if root folder was already saved
    if (currentRoot && cachedModels.length === 0) {
      void runScan();
    }
  },
};
