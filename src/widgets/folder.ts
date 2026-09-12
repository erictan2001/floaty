import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { addPinMenu, appWin, applyFloatieAnimation, currentSettings, loadRecord, monitorArea, removeSelf, saveRecord, watchPluginEnabled, watchSettings, type WidgetRecord } from "./lib";
import type { FloatyPlugin, PluginRecord } from "./plugin";

interface FolderItem {
  name: string;
  target: string;
  icon: string;
  is_dir?: boolean;
}

const WIN_W = 92;
const WIN_H = 116;
const CELL = 84;

export function mountFolder(root: HTMLElement, id: string): void {
  watchPluginEnabled("folder");
  const wrap = document.createElement("div");
  wrap.className = "folder";
  root.append(wrap);

  let rec: WidgetRecord | undefined;
  let pinArmed = false;
  let items: FolderItem[] = [];
  let expanded = false;
  let dragging = false;
  let suppressClickUntil = 0;
  let itemDragMoved = false;
  let scale = 1;
  let px = 200;
  let py = 200;
  let lastSavedX = -1;
  let lastSavedY = -1;

  watchSettings();

  const syncAnim = () => {
    applyFloatieAnimation(wrap, id);
  };
  const syncFloat = syncAnim;
  syncAnim();
  listen("floaty-settings-changed", syncAnim).catch(() => undefined);

  const savePos = () => {
    if (!rec) return;
    const curX = Math.round(px);
    const curY = Math.round(py);
    if (curX === lastSavedX && curY === lastSavedY) return;
    lastSavedX = curX;
    lastSavedY = curY;
    rec.x = curX;
    rec.y = curY;
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

  let collapsedPos: { x: number; y: number } | null = null;

  const expandFolder = async (): Promise<void> => {
    if (expanded) return;
    collapsedPos = { x: px, y: py };

    const cols = Math.min(4, Math.max(1, items.length));
    const rows = Math.max(1, Math.ceil(items.length / cols));
    let targetW = Math.max(cols * CELL + (rows > 4 ? 28 : 20), 220);
    let targetH = Math.min(56 + rows * 82, 420);

    try {
      const mon = await monitorArea();
      const MARGIN = 16;
      targetW = Math.min(targetW, Math.floor(mon.w - MARGIN * 2));
      targetH = Math.min(targetH, Math.floor(mon.h - MARGIN * 2));

      let newX = px;
      let newY = py;

      if (newX + targetW > mon.x + mon.w - MARGIN) {
        newX = mon.x + mon.w - MARGIN - targetW;
      }
      if (newX < mon.x + MARGIN) {
        newX = mon.x + MARGIN;
      }
      if (newY + targetH > mon.y + mon.h - MARGIN) {
        newY = mon.y + mon.h - MARGIN - targetH;
      }
      if (newY < mon.y + MARGIN) {
        newY = mon.y + MARGIN;
      }

      px = newX;
      py = newY;
      await appWin.setPosition(new PhysicalPosition(Math.round(px * scale), Math.round(py * scale)));
      await setWindowSize(targetW, targetH);
    } catch {
      /* ignore */
    }

    expanded = true;
    render();

    const missingIcons = items.some((it) => !it.is_dir && (!it.icon || it.icon === "none" || it.icon.length < 5000));
    if (missingIcons) {
      invoke("floaty_resolve_folder_icons", { folderId: id }).catch(() => undefined);
    }
  };

  const collapseFolder = async (): Promise<void> => {
    if (!expanded) return;
    expanded = false;

    if (collapsedPos) {
      px = collapsedPos.x;
      py = collapsedPos.y;
      collapsedPos = null;
    }

    try {
      const mon = await monitorArea();
      const MARGIN = 16;
      if (px + WIN_W > mon.x + mon.w - MARGIN) px = mon.x + mon.w - MARGIN - WIN_W;
      if (px < mon.x + MARGIN) px = mon.x + MARGIN;
      if (py + WIN_H > mon.y + mon.h - MARGIN) py = mon.y + mon.h - MARGIN - WIN_H;
      if (py < mon.y + MARGIN) py = mon.y + MARGIN;

      await appWin.setPosition(new PhysicalPosition(Math.round(px * scale), Math.round(py * scale)));
      await setWindowSize(WIN_W, WIN_H);
      savePos();
    } catch {
      /* ignore */
    }

    render();
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
            const newName = input.value.trim() || "Folder";
            rec.data["name"] = newName;
            void saveRecord(rec).catch(() => undefined);
            if (typeof rec.data["path"] === "string" && rec.data["path"]) {
              invoke("floaty_rename_folder_dir", { folderId: id, newName }).catch(() => undefined);
            }
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
        void collapseFolder();
      });
      head.append(dot, nm, shut);
      const grid = document.createElement("div");
      grid.className = "fgrid";
      const cols = Math.min(4, Math.max(1, items.length));
      grid.style.gridTemplateColumns = `repeat(${cols}, 1fr)`;
      if (items.length === 0) {
        const empty = document.createElement("div");
        empty.className = "fempty";
        empty.textContent = "drag app icons or files onto this folder";
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
        } else if (it.is_dir) {
          const glyph = document.createElement("div");
          glyph.className = "ffolder";
          glyph.style.transform = "scale(0.85)";
          b.append(glyph);
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
        // drag an item out past the window edge to unfloat it as its own icon
        b.addEventListener("pointerdown", (e) => {
          if (e.button !== 0) return;
          e.stopPropagation(); // not a folder-window drag
          itemDragMoved = false;
          try {
            b.setPointerCapture(e.pointerId);
          } catch {
            /* ignore */
          }
          const sx = e.screenX;
          const sy = e.screenY;
          let out = false;
          const onMove = (ev: PointerEvent) => {
            if (!out && Math.hypot(ev.screenX - sx, ev.screenY - sy) > 8) {
              out = true;
              itemDragMoved = true;
              b.classList.add("dragging-out");
              suppressClickUntil = performance.now() + 300;
            }
          };
          const onUp = (ev: PointerEvent) => {
            window.removeEventListener("pointermove", onMove);
            window.removeEventListener("pointerup", onUp);
            window.removeEventListener("pointercancel", onUp);
            try {
              if (b.hasPointerCapture(e.pointerId)) b.releasePointerCapture(e.pointerId);
            } catch {
              /* ignore */
            }
            b.classList.remove("dragging-out");
            if (!out) return; // plain tap: the click handler below launches
            const inside =
              ev.clientX >= 0 &&
              ev.clientY >= 0 &&
              ev.clientX < window.innerWidth &&
              ev.clientY < window.innerHeight;
            if (inside) return; // dragged but stayed inside: cancel, don't launch
            const nx = Math.round(px + ev.clientX - 46);
            const ny = Math.round(py + ev.clientY - 56);
            const index = items.indexOf(it);
            invoke("floaty_ungroup", { folderId: id, index, x: nx, y: ny }).catch(
              () => undefined,
            );
          };
          window.addEventListener("pointermove", onMove);
          window.addEventListener("pointerup", onUp);
          window.addEventListener("pointercancel", onUp);
        });
        b.addEventListener("click", (e) => {
          e.stopPropagation();
          if (itemDragMoved) { itemDragMoved = false; return; } // was a drag, not a tap
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
      const targetW = Math.max(cols * CELL + (rows > 4 ? 28 : 20), 220);
      const targetH = Math.min(56 + rows * 82, 420);
      void setWindowSize(targetW, targetH);
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
    syncFloat();
    render();
  };

  void (async () => {
    await reload();
    // belt and braces with the builder flag: folders live under real apps
    void appWin.setAlwaysOnTop(false).catch(() => undefined);
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
    // In open mode, scrolling or interacting with the item grid shouldn't drag the window
    if (expanded && t.closest(".fgrid")) return;
    // buttons and text fields handle themselves (rename input stays usable)
    if (t.closest("button, input, textarea")) return;
    e.stopPropagation();
    dragging = true;
    void appWin.scaleFactor().then((s) => { if (s > 0) scale = s; }).catch(() => undefined);
    // capture lazily on first real movement (see pet.ts: eager capture eats taps)
    let captured = false;
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
        if (!captured) {
          captured = true;
          try {
            wrap.setPointerCapture(e.pointerId);
          } catch {
            /* ignore */
          }
        }
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
      if (moved) {
        void savePos();
        if (expanded) {
          collapsedPos = { x: px, y: py };
        }
        invoke("floaty_dropped", { id }).catch(() => undefined);
      }
      dragging = false;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  });
  wrap.addEventListener("click", (e) => {
    e.stopPropagation();
    if (dragging || performance.now() < suppressClickUntil) return;
    // collapse only via the – button, so clicks inside the open panel
    // (rename field included) never fold it away by accident
    if (expanded) return;
    void expandFolder();
  });
  wrap.addEventListener("contextmenu", (e) => e.preventDefault());
}

export const folderPlugin: FloatyPlugin = {
  kind: "folder",
  name: "Folder",
  addLabel: "+ folder",
  mount: mountFolder,
  describe: (rec: PluginRecord) => {
    const raw = rec.data["items"];
    const n = Array.isArray(raw) ? raw.length : 0;
    const nm = typeof rec.data["name"] === "string" && rec.data["name"] ? rec.data["name"] : "folder";
    const path = typeof rec.data["path"] === "string" ? rec.data["path"] : "";
    return path ? `${nm} (${n} item${n === 1 ? "" : "s"}) — ${path}` : `${nm} (${n} item${n === 1 ? "" : "s"})`;
  },
};
