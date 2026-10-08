import { invoke } from "@tauri-apps/api/core";
import { pathOf } from "./pluginManifest";
import { appWin, isOverlayMode, WidgetRecord } from "./windowState";
import { scheduleHitRectsUpdate } from "./slots";
import { hasDesktopPath, removeSelf, saveRecord } from "./records";

export interface PinMenuApi {
  run: (label: string, fn: () => unknown) => void;
  row: (label: string) => HTMLElement;
  divider: () => void;
  note: (msg: string) => void;
  close: () => void;
  swap: (fill: (body: HTMLElement, done: () => void) => void) => void;
}

export interface PinMenuOptions {
  rows?: (api: PinMenuApi) => void;
}

export function enforceDesktopLayer(): void {
  if (isOverlayMode()) return;
  void appWin.setAlwaysOnTop(false).catch(() => undefined);
}

export function addPinMenu(
  wrap: HTMLElement,
  getRec: () => WidgetRecord | undefined,
  options?: PinMenuOptions,
): void {
  wrap.addEventListener("contextmenu", (e) => {
    const t = e.target as HTMLElement | null;
    if (t instanceof Element && t.closest("textarea, input")) return;
    e.preventDefault();
    e.stopPropagation();
    document.querySelectorAll(".pin-menu").forEach((m) => m.remove());
    const rec = getRec();
    if (!rec) return;
    const menu = document.createElement("div");
    menu.className = "pin-menu";

    const closeMenu = (): void => {
      menu.remove();
      scheduleHitRectsUpdate();
    };
    const note = (msg: string): void => {
      const line = document.createElement("div");
      line.className = "pin-note";
      line.textContent = msg;
      menu.prepend(line);
      window.setTimeout(() => line.remove(), 2200);
    };
    const addRow = (label: string, danger = false): HTMLElement => {
      const row = document.createElement("div");
      row.className = danger ? "pin-row pin-btn danger" : "pin-row pin-btn";
      row.textContent = label;
      menu.append(row);
      return row;
    };
    const run = (label: string, fn: () => Promise<unknown>, danger = false): void => {
      const row = addRow(label, danger);
      row.addEventListener("click", (ev) => {
        ev.stopPropagation();
        closeMenu();
        void fn().catch((err: unknown) => {
          void invoke("floaty_log", { msg: `[menu] ${label} failed: ${String(err)}` }).catch(
            () => undefined,
          );
        });
      });
    };
    const divider = (): void => {
      const hr = document.createElement("div");
      hr.className = "pin-sep";
      menu.append(hr);
    };

    options?.rows?.({
      run: (label, fn) => {
        run(label, () => Promise.resolve(fn()));
      },
      row: (label) => addRow(label),
      divider,
      note,
      close: closeMenu,
      swap: (fill) => {
        menu.textContent = "";
        fill(menu, closeMenu);
        scheduleHitRectsUpdate();
      },
    });

    const path = pathOf(rec);
    const isFileish = path !== "" && hasDesktopPath(rec);
    if (isFileish) {
      run("Open", () => invoke("floaty_launch", { id: rec.id }));
      run("Open with…", () => invoke("floaty_open_with", { id: rec.id }));
      run("Show in File Explorer", () => invoke("floaty_reveal", { id: rec.id }));
      const renameRow = addRow("Rename…");
      renameRow.addEventListener("click", (ev) => {
        ev.stopPropagation();
        void startRename(menu, rec, note).catch(() => {
          closeMenu();
        });
      });
      run("Copy path", async () => {
        await navigator.clipboard.writeText(path);
        note("path copied");
      });
      run("Properties", () => invoke("floaty_properties", { id: rec.id }));
      divider();
    }

    const label = document.createElement("label");
    label.className = "pin-row";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = rec.data["on_top"] === true;
    box.addEventListener("click", (ev) => ev.stopPropagation());
    box.addEventListener("change", () => {
      const onTop = box.checked;
      rec.data["on_top"] = onTop;
      void invoke("floaty_set_on_top", { id: rec.id, onTop }).catch((err: unknown) => {
        void invoke("floaty_log", {
          msg: `[menu] pin on top ${rec.id} failed: ${String(err)}`,
        }).catch(() => undefined);
      });
      void saveRecord(rec).catch(() => undefined);
      closeMenu();
    });
    const txt = document.createElement("span");
    txt.textContent = "Pin on top";
    label.append(box, txt);
    menu.append(label);

    if (isFileish) {
      divider();
      run("Delete (Recycle Bin)", () => removeSelf(rec), true);
    }

    const menuW = 216;
    menu.style.left = `${Math.max(4, Math.min(e.clientX, window.innerWidth - menuW))}px`;
    menu.style.top = `${Math.max(4, Math.min(e.clientY, window.innerHeight - 48))}px`;
    document.body.append(menu);
    scheduleHitRectsUpdate();
    window.setTimeout(() => {
      const dismiss = (ev: PointerEvent) => {
        if (!menu.contains(ev.target as Node)) {
          menu.remove();
          scheduleHitRectsUpdate();
          window.removeEventListener("pointerdown", dismiss, true);
        }
      };
      window.addEventListener("pointerdown", dismiss, true);
    }, 0);
  });
}

function startRename(
  menu: HTMLElement,
  rec: WidgetRecord,
  note: (msg: string) => void,
): Promise<void> {
  return new Promise<void>((resolve) => {
    const current = pathOf(rec);
    const base = current.split(/[\\/]/).pop() ?? "";
    menu.textContent = "";
    const input = document.createElement("input");
    input.className = "pin-input";
    input.value = base;
    menu.append(input);
    const hint = document.createElement("div");
    hint.className = "pin-note";
    hint.textContent = "enter to rename · esc to cancel";
    menu.append(hint);
    input.addEventListener("pointerdown", (ev) => ev.stopPropagation());
    input.focus();
    const dot = base.lastIndexOf(".");
    if (dot > 0) input.setSelectionRange(0, dot);
    else input.select();

    let done = false;
    let busy = false;
    const cleanup = (): void => {
      done = true;
      menu.remove();
      scheduleHitRectsUpdate();
      resolve();
    };
    const finish = (commit: boolean): void => {
      if (done || busy) return;
      const name = input.value.trim();
      if (!commit || !name || name === base) {
        cleanup();
        return;
      }
      busy = true;
      input.disabled = true;
      invoke<WidgetRecord>("floaty_rename", { id: rec.id, name })
        .then(cleanup)
        .catch((err: unknown) => {
          busy = false;
          input.disabled = false;
          input.focus();
          note(String(err));
          void invoke("floaty_log", { msg: `[menu] rename failed: ${String(err)}` }).catch(
            () => undefined,
          );
        });
    };
    input.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter") finish(true);
      else if (ev.key === "Escape") finish(false);
    });
    input.addEventListener("blur", () => {
      if (!done) finish(true);
    });
  });
}
