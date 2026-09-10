import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { addPinMenu, appWin, loadRecord, removeSelf, saveRecord, type WidgetRecord } from "./lib";

interface FolderItem {
  name: string;
  target: string;
  icon: string;
}

const WIN_W = 92;
const WIN_H = 112;
const CELL = 84;

export function mountFolder(root: HTMLElement, id: string): void {
  const wrap = document.createElement("div");
  wrap.className = "folder";
  root.append(wrap);

  let rec: WidgetRecord | undefined;
  let pinArmed = false;
  let items: FolderItem[] = [];
  let expanded = false;
  let dragging = false;
  let suppressClickUntil = 0;
  let scale = 1;
  let px = 200;
  let py = 200;

  const savePos = () => {
    if (!rec) return;
    rec.x = Math.round(px);
    rec.y = Math.round(py);
    void saveRecord(rec).catch(() => undefined);
  };

  const folderName = (): string =>
    rec && typeof rec.data["name"] === "string" && (rec.data["name"] as string)
      ? (rec.data["name"] as string)
      : "Folder";

  const setWindowSize = async (w: number, h: number): Promise<void> => {
    try {
      const s = await appWin.scaleFactor();
      const k = s > 0 ? s : 1;
      scale = k;
      await appWin.setSize(new PhysicalSize(Math.round(w * k), Math.round(h * k)));
    } catch {
      /* ignore */
    }
  };

  const render = () => {
    wrap.innerHTML = "";
    wrap.classList.toggle("open", expanded);
    if (!expanded) {
      const tile = document.createElement("div");
      tile.className = "ftile";
      // android-style preview: up to 4 mini icons packed in the folder
      const shown = items.slice(0, 4);
      if (shown.length === 0) {
        const glyph = document.createElement("div");
        glyph.className = "ffolder";
        tile.append(glyph);
      } else {
        const minis = document.createElement("div");
        minis.className = "fminis";
        for (const it of shown) {
          if (it.icon) {
            const img = document.createElement("img");
            img.src = it.icon;
            img.alt = "";
            img.draggable = false;
            minis.append(img);
          } else {
            const ch = document.createElement("span");
            ch.className = "fmini-letter";
            ch.textContent = (it.name.trim()[0] ?? "?").toUpperCase();
            minis.append(ch);
          }
        }
        tile.append(minis);
      }
      if (items.length > 0) {
        const badge = document.createElement("div");
        badge.className = "fbadge";
        badge.textContent = String(items.length);
        tile.append(badge);
      }
      const x = document.createElement("button");
      x.className = "launcher-x";
      x.title = "Remove";
      x.textContent = "×";
      x.addEventListener("pointerdown", (e) => e.stopPropagation());
      x.addEventListener("click", (e) => {
        e.stopPropagation();
        if (rec) void removeSelf(rec);
      });
      const nm = document.createElement("div");
      nm.className = "launcher-name";
      nm.textContent = folderName();
      nm.title = folderName();
      wrap.append(tile, x, nm);
      void setWindowSize(WIN_W, WIN_H);
    } else {
      const head = document.createElement("div");
      head.className = "fhead";
      const dot = document.createElement("span");
      dot.className = "fhead-dot";
      const nm = document.createElement("span");
      nm.className = "fhead-name";
      nm.textContent = folderName();
      nm.title = "Click to rename";
      nm.style.cursor = "text";
      nm.addEventListener("click", (e) => {
        e.stopPropagation();
        const input = document.createElement("input");
        input.className = "fname-edit";
        input.value = folderName();
        input.maxLength = 24;
        const commit = (save: boolean) => {
          if (save && rec) {
            rec.data["name"] = input.value.trim() || "Folder";
            void saveRecord(rec).catch(() => undefined);
          }
          render();
        };
        input.addEventListener("keydown", (ev) => {
          if (ev.key === "Enter") commit(true);
          else if (ev.key === "Escape") commit(false);
        });
        input.addEventListener("blur", () => commit(true));
        input.addEventListener("pointerdown", (ev) => ev.stopPropagation());
        input.addEventListener("click", (ev) => ev.stopPropagation());
        head.replaceChild(input, nm);
        input.focus();
        input.select();
      });
      const shut = document.createElement("button");
      shut.className = "fshut";
      shut.title = "Collapse";
      shut.textContent = "–";
      shut.addEventListener("pointerdown", (e) => e.stopPropagation());
      shut.addEventListener("click", (e) => {
        e.stopPropagation();
        expanded = false;
        render();
      });
      head.append(dot, nm, shut);
      const grid = document.createElement("div");
      grid.className = "fgrid";
      const cols = Math.min(4, Math.max(1, items.length));
      grid.style.gridTemplateColumns = `repeat(${cols}, 1fr)`;
      if (items.length === 0) {
        const empty = document.createElement("div");
        empty.className = "fempty";
        empty.textContent = "drag app icons onto this folder";
        grid.append(empty);
      }
      for (const it of items) {
        const b = document.createElement("button");
        b.className = "fitem";
        b.title = it.name;
        if (it.icon) {
          const img = document.createElement("img");
          img.src = it.icon;
          img.alt = "";
          img.draggable = false;
          b.append(img);
        } else {
          const ch = document.createElement("span");
          ch.className = "fletter";
          ch.textContent = (it.name.trim()[0] ?? "?").toUpperCase();
          b.append(ch);
        }
        const lab = document.createElement("span");
        lab.className = "flabel";
        lab.textContent = it.name;
        b.append(lab);
        b.addEventListener("click", (e) => {
          e.stopPropagation();
          b.classList.add("go");
          window.setTimeout(() => b.classList.remove("go"), 500);
          invoke("floaty_launch_target", { target: it.target }).catch(() => {
            b.classList.remove("go");
            b.classList.add("shake");
            window.setTimeout(() => b.classList.remove("shake"), 500);
          });
        });
        grid.append(b);
      }
      wrap.append(head, grid);
      const rows = Math.max(1, Math.ceil(items.length / cols));
      void setWindowSize(Math.max(cols * CELL + 20, 220), 64 + rows * 100 + 16);
    }
  };

  const reload = async (): Promise<void> => {
    const r = await loadRecord(id);
    if (!r) {
      wrap.remove();
      return;
    }
    rec = r;
    if (!pinArmed) {
      pinArmed = true;
      addPinMenu(wrap, () => rec);
    }
    const raw = r.data["items"];
    items = Array.isArray(raw)
      ? (raw as FolderItem[]).filter((it) => it && typeof it.target === "string")
      : [];
    try {
      const p = await appWin.outerPosition();
      const s = await appWin.scaleFactor();
      const k = s > 0 ? s : 1;
      scale = k;
      px = p.x / k;
      py = p.y / k;
    } catch {
      /* keep */
    }
    render();
  };

  void (async () => {
    await reload();
    // belt and braces with the builder flag: folders live under real apps
    void appWin.setAlwaysOnTop(false).catch(() => undefined);
    window.setInterval(() => void savePos(), 4000);
    window.addEventListener("beforeunload", () => void savePos());
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) void savePos();
    });
    await listen<string>("floaty-folder-changed", (e) => {
      if (e.payload === id) void reload();
    }).catch(() => undefined);
    void appWin.show().catch(() => undefined);
  })();

  // manual pinned drag (no gravity); a tap toggles expand
  wrap.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || dragging) return;
    const t = e.target as HTMLElement;
    if (t.closest("button")) return; // × / collapse / items handle themselves
    e.stopPropagation();
    dragging = true;
    try {
      wrap.setPointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
    const startX = px;
    const startY = py;
    const startSX = e.screenX;
    const startSY = e.screenY;
    let moved = false;
    const onMove = (ev: PointerEvent) => {
      px = startX + (ev.screenX - startSX);
      py = startY + (ev.screenY - startSY);
      if (Math.hypot(ev.screenX - startSX, ev.screenY - startSY) > 4) {
        moved = true;
        suppressClickUntil = performance.now() + 300;
      }
      void appWin
        .setPosition(new PhysicalPosition(Math.round(px * scale), Math.round(py * scale)))
        .catch(() => undefined);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      try {
        if (wrap.hasPointerCapture(e.pointerId)) wrap.releasePointerCapture(e.pointerId);
      } catch {
        /* ignore */
      }
      if (moved) void savePos();
      dragging = false;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  });
  wrap.addEventListener("click", (e) => {
    e.stopPropagation();
    if (dragging || performance.now() < suppressClickUntil) return;
    expanded = !expanded;
    render();
  });
  wrap.addEventListener("contextmenu", (e) => e.preventDefault());
}
