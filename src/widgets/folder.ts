import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { addPinMenu, appWin, applyFloatieAnimation, currentSettings, displayName, dragGrab, dragPosition, enforceDesktopLayer, iconIsMissing, isOnMyScreen, isOverlayMode, loadRecord, logicalPos, monitorArea, notifyDragMove, notifyDragging, onSettings, removeSelf, saveRecord, setWidgetPos, setWidgetSize, watchPluginEnabled, watchSettings, type WidgetRecord } from "./lib";
import type { FloatyPlugin, PluginRecord } from "./plugin";

interface FolderItem {
  name: string;
  target: string;
  icon: string;
  is_dir?: boolean;
}

const WIN_W = 92;
const WIN_H = 112;
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
    // px/py are the folder's place on the desktop, which is its place in the
    // wave (see applyFloatieAnimation).
    applyFloatieAnimation(wrap, id, { x: px, y: py });
  };
  const syncFloat = syncAnim;
  onSettings(syncAnim);

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
      setWidgetSize(id, w, h, k);
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
      setWidgetPos(id, px, py, scale);
      await setWindowSize(targetW, targetH);
    } catch {
      /* ignore */
    }

    expanded = true;
    wrap.parentElement?.style.setProperty("z-index", "100");
    render();

    // Directories included: excluding them left subfolders blank until the app
    // was restarted, because the startup pass was the only thing resolving them.
    const missingIcons = items.some((it) => iconIsMissing(it.icon));
    if (missingIcons) {
      invoke("floaty_resolve_folder_icons", { folderId: id }).catch(() => undefined);
    }
  };

  const collapseFolder = async (): Promise<void> => {
    if (!expanded) return;
    expanded = false;
    wrap.parentElement?.style.removeProperty("z-index");

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

      setWidgetPos(id, px, py, scale);
      await setWindowSize(WIN_W, WIN_H);
      savePos();
    } catch {
      /* ignore */
    }

    render();
  };

  /** What an item shows when it has no icon, or the one it has will not load. */
  const fallbackVisual = (it: FolderItem, mini: boolean): HTMLElement => {
    if (it.is_dir) {
      const glyph = document.createElement("div");
      glyph.className = mini ? "ffolder fmini-dir" : "ffolder";
      if (!mini) glyph.style.transform = "scale(0.85)";
      return glyph;
    }
    const ch = document.createElement("span");
    ch.className = mini ? "fmini-letter" : "fletter";
    ch.textContent = (it.name.trim()[0] ?? "?").toUpperCase();
    return ch;
  };

  const createItemVisual = (it: FolderItem, mini: boolean): HTMLElement => {
    if (it.icon) {
      const img = document.createElement("img");
      img.src = it.icon;
      img.alt = "";
      img.draggable = false;
      // An icon is a file; if it is gone the item keeps its place with the
      // letter (or the folder glyph) rather than an image that never loads.
      // `floaty_resolve_folder_icons` re-resolves it on the folder's next open.
      img.onerror = () => img.replaceWith(fallbackVisual(it, mini));
      return img;
    }
    return fallbackVisual(it, mini);
  };

  const createGhostElement = (it: FolderItem, clientX: number, clientY: number): HTMLElement => {
    const ghost = document.createElement("div");
    ghost.className = "floaty-drag-ghost";
    ghost.style.transform = `translate3d(${clientX - 36}px, ${clientY - 36}px, 0)`;
    if (it.icon && it.icon !== "none") {
      const gImg = document.createElement("img");
      gImg.src = it.icon;
      ghost.append(gImg);
    } else if (it.is_dir) {
      const gGlyph = document.createElement("div");
      gGlyph.className = "ffolder ghost-folder";
      ghost.append(gGlyph);
    } else {
      const gTile = document.createElement("div");
      gTile.className = "ghost-letter";
      gTile.textContent = (it.name.trim()[0] ?? "?").toUpperCase();
      ghost.append(gTile);
    }
    const gLab = document.createElement("span");
    gLab.className = "ghost-label";
    gLab.textContent = it.name;
    ghost.append(gLab);
    return ghost;
  };

  const renderCollapsed = () => {
    const tile = document.createElement("div");
    tile.className = "ftile";
    const shown = items.slice(0, 4);
    if (shown.length === 0) {
      const glyph = document.createElement("div");
      glyph.className = "ffolder";
      tile.append(glyph);
    } else {
      const minis = document.createElement("div");
      minis.className = "fminis";
      for (const it of shown) {
        minis.append(createItemVisual(it, true));
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
  };

  const setupItemDrag = (b: HTMLButtonElement, it: FolderItem) => {
    b.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.stopPropagation();
      itemDragMoved = false;
      const isOverlay = isOverlayMode();
      const sx = isOverlay ? e.clientX : e.screenX;
      const sy = isOverlay ? e.clientY : e.screenY;
      let out = false;
      let captured = false;
      let ghost: HTMLElement | null = null;

      const onMove = (ev: PointerEvent) => {
        const curSX = isOverlay ? ev.clientX : ev.screenX;
        const curSY = isOverlay ? ev.clientY : ev.screenY;
        if (!out && Math.hypot(curSX - sx, curSY - sy) > 8) {
          out = true;
          itemDragMoved = true;
          b.classList.add("dragging-out");
          suppressClickUntil = performance.now() + 300;
          notifyDragging(true);
          if (!captured) {
            captured = true;
            try {
              b.setPointerCapture(e.pointerId);
            } catch {
              /* ignore */
            }
          }
          ghost = createGhostElement(it, ev.clientX, ev.clientY);
          document.body.append(ghost);
        }
        if (ghost) {
          ghost.style.transform = `translate3d(${ev.clientX - 36}px, ${ev.clientY - 36}px, 0)`;
        }
      };

      const onUp = (ev: PointerEvent) => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("pointercancel", onUp);
        if (captured) {
          try {
            if (b.hasPointerCapture(e.pointerId)) b.releasePointerCapture(e.pointerId);
          } catch {
            /* ignore */
          }
        }
        if (ghost) {
          ghost.remove();
          ghost = null;
        }
        b.classList.remove("dragging-out");
        if (out) notifyDragging(false);
        if (!out) return;

        const isInside = isOverlay
          ? (() => {
              const r = wrap.getBoundingClientRect();
              return (
                ev.clientX >= r.left + 4 &&
                ev.clientX <= r.right - 4 &&
                ev.clientY >= r.top + 4 &&
                ev.clientY <= r.bottom - 4
              );
            })()
          : ev.clientX >= 0 &&
            ev.clientY >= 0 &&
            ev.clientX < window.innerWidth &&
            ev.clientY < window.innerHeight;

        if (isInside) return;

        let nx = isOverlay ? Math.round(ev.clientX - 46) : Math.round(px + ev.clientX - 46);
        let ny = isOverlay ? Math.round(ev.clientY - 56) : Math.round(py + ev.clientY - 56);
        if (isOverlay) {
          const monW = window.innerWidth || 1920;
          const monH = window.innerHeight || 1080;
          nx = Math.max(16, Math.min(monW - 92 - 16, nx));
          ny = Math.max(16, Math.min(monH - 112 - 16, ny));
        }
        const index = items.indexOf(it);
        if (index >= 0) {
          invoke("floaty_ungroup", { folderId: id, index, x: nx, y: ny })
            .then(() => reload())
            .catch(() => undefined);
        }
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onUp);
    });
  };

  const renderExpanded = () => {
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
          const onDisk = typeof rec.data["path"] === "string" && rec.data["path"];
          if (onDisk) {
            // One writer: for a folder that is a real directory the disk rename *is* the
            // rename — it writes the new name into the record and announces it, and a
            // refusal (a name that is taken, one Windows will not accept) leaves the tile
            // saying what the folder is really called instead of saving a name it never got.
            void invoke("floaty_rename_folder_dir", { folderId: id, newName }).catch((err: unknown) => {
              void invoke("floaty_log", { msg: `[folder] rename refused: ${String(err)}` }).catch(() => undefined);
            });
          } else {
            // A folder made only of items exists in its record; there is nowhere else its
            // name could live.
            rec.data["name"] = newName;
            void saveRecord(rec).catch(() => undefined);
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
      b.append(createItemVisual(it, false));
      const lab = document.createElement("span");
      lab.className = "flabel";
      lab.textContent = displayName(it.name);
      b.append(lab);
      setupItemDrag(b, it);
      b.addEventListener("click", (e) => {
        e.stopPropagation();
        if (itemDragMoved) {
          itemDragMoved = false;
          return;
        }
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
  };

  const render = () => {
    wrap.innerHTML = "";
    wrap.classList.toggle("open", expanded);
    if (!expanded) {
      renderCollapsed();
    } else {
      renderExpanded();
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
      const p = await logicalPos(id);
      px = p.x;
      py = p.y;
    } catch {
      px = r.x;
      py = r.y;
    }
    syncFloat();
    render();
  };

  void (async () => {
    await reload();
    // belt and braces with the builder flag: folders live under real apps
    enforceDesktopLayer();
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
    notifyDragging(true);
    void invoke("floaty_gesture_begin", { label: "move", ids: [id] }).catch(() => undefined);
    // same held state the app tile uses: it is what lets the merge preview
    // shrink the dragged folder as it hovers another tile
    wrap.classList.add("held");
    void appWin.scaleFactor().then((s) => { if (s > 0) scale = s; }).catch(() => undefined);
    // capture lazily on first real movement (eager capture eats taps)
    let captured = false;
    const isOverlay = isOverlayMode();
    const startX = px;
    const startY = py;
    const startSX = isOverlay ? e.clientX : e.screenX;
    const startSY = isOverlay ? e.clientY : e.screenY;
    let moved = false;
    // Where the pointer holds the tile, and the screens it can be mapped through — a
    // folder drags onto another screen the same way an icon does.
    const grab = isOverlay ? dragGrab(e, startX, startY) : null;
    const onMove = (ev: PointerEvent) => {
      const curSX = isOverlay ? ev.clientX : ev.screenX;
      const curSY = isOverlay ? ev.clientY : ev.screenY;
      const want = isOverlay ? dragPosition(ev) : null;
      if (want && grab) {
        px = want.x - grab.dx;
        py = want.y - grab.dy;
        // every move, so a drag that crosses back is handed back (see appicon.ts)
        void invoke("floaty_drag_to", { id, x: Math.round(px), y: Math.round(py) }).catch(
          () => undefined,
        );
      } else {
        px = startX + (curSX - startSX);
        py = startY + (curSY - startSY);
      }
      if (Math.hypot(curSX - startSX, curSY - startSY) > 4) {
        moved = true;
        suppressClickUntil = performance.now() + 300;
        if (!captured) {
          captured = true;
          try {
            // on the body: the tile's slot is removed when the drag hands it to another
            // screen's window, and a capture on a removed element stops delivering moves
            (document.body ?? wrap).setPointerCapture(e.pointerId);
          } catch {
            /* ignore */
          }
        }
      }
      setWidgetPos(id, px, py, scale);
      if (isOnMyScreen(px, py)) notifyDragMove(id, px, py);
      else window.dispatchEvent(new CustomEvent("floaty-drag-end"));
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      void invoke("floaty_gesture_end").catch(() => undefined);
      try {
        const held = (document.body ?? wrap) as HTMLElement;
        if (held.hasPointerCapture(e.pointerId)) held.releasePointerCapture(e.pointerId);
      } catch {
        /* ignore */
      }
      wrap.classList.remove("held");
      if (moved) {
        void (async () => {
          try {
            const res = await invoke<string | null>("floaty_dropped", {
              id,
              x: Math.round(px),
              y: Math.round(py),
            });
            if (res) return;
          } catch {
            /* ignore */
          }

          savePos();
          if (expanded) {
            collapsedPos = { x: px, y: py };
          }
        })();
      }
      dragging = false;
      notifyDragging(false);
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
  mount: mountFolder,
  describe: (rec: PluginRecord) => {
    const raw = rec.data["items"];
    const n = Array.isArray(raw) ? raw.length : 0;
    const nm = typeof rec.data["name"] === "string" && rec.data["name"] ? rec.data["name"] : "folder";
    const path = typeof rec.data["path"] === "string" ? rec.data["path"] : "";
    return path ? `${nm} (${n} item${n === 1 ? "" : "s"}) — ${path}` : `${nm} (${n} item${n === 1 ? "" : "s"})`;
  },
};
