import { invoke } from "@tauri-apps/api/core";
import { currentMonitor, getCurrentWindow, PhysicalPosition } from "@tauri-apps/api/window";

export interface WidgetRecord {
  id: string;
  kind: string;
  /** logical px — matches Rust builder + store */
  x: number;
  y: number;
  data: Record<string, unknown>;
}

export const appWin = getCurrentWindow();

async function scaleFactor(): Promise<number> {
  try {
    const s = await appWin.scaleFactor();
    return s > 0 ? s : 1;
  } catch {
    return 1;
  }
}

/** Window position in logical px. */
export async function logicalPos(): Promise<{ x: number; y: number }> {
  const s = await scaleFactor();
  const p = await appWin.outerPosition();
  return { x: p.x / s, y: p.y / s };
}

export async function setLogicalPos(x: number, y: number): Promise<void> {
  const s = await scaleFactor();
  await appWin.setPosition(new PhysicalPosition(Math.round(x * s), Math.round(y * s)));
}

export interface MonitorArea {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Current monitor work area in logical px. */
export async function monitorArea(): Promise<MonitorArea> {
  const s = await scaleFactor();
  try {
    const m = await currentMonitor();
    if (m) {
      return {
        x: m.position.x / s,
        y: m.position.y / s,
        w: m.size.width / s,
        h: m.size.height / s,
      };
    }
  } catch {
    /* fall through */
  }
  return { x: 0, y: 0, w: 1280, h: 800 };
}

export async function loadRecord(id: string): Promise<WidgetRecord | undefined> {
  const list = await invoke<WidgetRecord[]>("floaty_list");
  return list.find((r) => r.id === id);
}

export async function saveRecord(rec: WidgetRecord): Promise<void> {
  await invoke("floaty_save", { record: rec });
}

export function debounce<F extends (...args: never[]) => void>(
  fn: F,
  ms: number,
): (...args: Parameters<F>) => void {
  let t: number | undefined;
  return (...args: Parameters<F>) => {
    window.clearTimeout(t);
    t = window.setTimeout(() => fn(...args), ms);
  };
}

/** Persist window position (logical px) every 2s + on unload. */
export function trackPosition(rec: WidgetRecord): void {
  const snap = async () => {
    try {
      const p = await logicalPos();
      const x = Math.round(p.x);
      const y = Math.round(p.y);
      if (x !== rec.x || y !== rec.y) {
        rec.x = x;
        rec.y = y;
        await saveRecord(rec);
      }
    } catch {
      /* window gone */
    }
  };
  window.setInterval(() => void snap(), 2000);
  window.addEventListener("beforeunload", () => void snap());
  // persist the moment a drag ends or the window hides instead of waiting
  // for the next poll — quits otherwise lose the last move
  window.addEventListener("pointerup", () => window.setTimeout(() => void snap(), 60));
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) void snap();
  });
}

/** Title bar that native-drags the window. Buttons inside must stopPropagation. */
export function makeBar(title: string, onClose: () => void): HTMLElement {
  const bar = document.createElement("div");
  bar.className = "bar";
  const dots = document.createElement("span");
  dots.className = "dots";
  const label = document.createElement("span");
  label.className = "bar-title";
  label.textContent = title;
  const close = document.createElement("button");
  close.className = "icon-btn";
  close.textContent = "\u00d7";
  close.title = "Remove widget";
  close.addEventListener("pointerdown", (e) => e.stopPropagation());
  close.addEventListener("click", (e) => {
    e.stopPropagation();
    onClose();
  });
  bar.append(dots, label, close);
  bar.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    void appWin.startDragging().catch(() => undefined);
  });
  return bar;
}

export async function removeSelf(rec: WidgetRecord): Promise<void> {
  try {
    await invoke("floaty_remove", { id: rec.id });
  } catch {
    await appWin.close().catch(() => undefined);
  }
}
