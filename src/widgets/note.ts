import { PhysicalSize } from "@tauri-apps/api/window";
import { addPinMenu, addResizeHandle, appWin, debounce, loadRecord, makeBar, removeSelf, saveRecord, trackPosition } from "./lib";

export function mountNote(root: HTMLElement, id: string): void {
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
    wrap.prepend(makeBar("floaty note", () => void removeSelf(rec)));
    addPinMenu(wrap, () => rec);
    // restore saved size
    const w = typeof rec.data["w"] === "number" ? (rec.data["w"] as number) : 0;
    const h = typeof rec.data["h"] === "number" ? (rec.data["h"] as number) : 0;
    if (w > 100 && h > 100) {
      try {
        const s = await appWin.scaleFactor();
        const k = s > 0 ? s : 1;
        await appWin.setSize(
          new PhysicalSize(Math.round(Math.min(w, 1400) * k), Math.round(Math.min(h, 1400) * k)),
        );
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
