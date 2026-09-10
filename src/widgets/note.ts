import { debounce, loadRecord, makeBar, removeSelf, saveRecord, trackPosition } from "./lib";

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
    area.value = typeof rec.data["text"] === "string" ? (rec.data["text"] as string) : "";
    const persist = debounce(() => {
      rec.data["text"] = area.value;
      void saveRecord(rec);
    }, 400);
    area.addEventListener("input", persist);
    trackPosition(rec);
  })();
}
