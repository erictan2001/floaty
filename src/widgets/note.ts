import { addPinMenu, addResizeHandle, appWin, debounce, loadRecord, makeBar, removeSelf, saveRecord, setWidgetSize, trackPosition, watchPluginEnabled } from "./lib";
import type { FloatyPlugin, PluginRecord } from "./plugin";

export function mountNote(root: HTMLElement, id: string): void {
  watchPluginEnabled("note");
  const wrap = document.createElement("div");
  wrap.className = "bubble note";
  const area = document.createElement("textarea");
  area.className = "note-area";
  area.placeholder = "let it float...";
  wrap.append(area);
  root.append(wrap);

  void (async () => {
    const rec = await loadRecord(id);
    if (!rec) return;
    wrap.prepend(makeBar("floaty note", () => void removeSelf(rec), id));
    addPinMenu(wrap, () => rec);
    // restore saved size
    const w = typeof rec.data["w"] === "number" ? (rec.data["w"] as number) : 0;
    const h = typeof rec.data["h"] === "number" ? (rec.data["h"] as number) : 0;
    if (w > 100 && h > 100) {
      try {
        const s = await appWin.scaleFactor();
        const k = s > 0 ? s : 1;
        setWidgetSize(id, Math.min(w, 1400), Math.min(h, 1400), k);
      } catch {
        /* keep default size */
      }
    }
    addResizeHandle(wrap, rec, 180, 140);
    area.value = typeof rec.data["text"] === "string" ? (rec.data["text"] as string) : "";
    const persist = debounce(() => {
      rec.data["text"] = area.value;
      void saveRecord(rec);
    }, 400);
    area.addEventListener("input", persist);
    trackPosition(rec);
  })();
}

export const notePlugin: FloatyPlugin = {
  kind: "note",
  name: "Note",
  addLabel: "+ note",
  mount: mountNote,
  describe: (rec: PluginRecord) => {
    const t = rec.data["text"];
    return typeof t === "string" && t ? t.slice(0, 32) : undefined;
  },
};
