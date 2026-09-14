import { addPinMenu, addResizeHandle, appWin, loadRecord, makeBar, removeSelf, saveRecord, setWidgetSize, trackPosition, watchPluginEnabled, type WidgetRecord } from "./lib";
import type { FloatyPlugin } from "./plugin";

const DEFAULT_FOCUS_S = 25 * 60;
const DEFAULT_BREAK_S = 5 * 60;

function fmt(total: number): string {
  const m = Math.floor(total / 60).toString().padStart(2, "0");
  const s = (total % 60).toString().padStart(2, "0");
  return `${m}:${s}`;
}

export function mountClock(root: HTMLElement, id: string): void {
  watchPluginEnabled("clock");
  const wrap = document.createElement("div");
  wrap.className = "bubble clock";

  const time = document.createElement("div");
  time.className = "clock-time";
  const date = document.createElement("div");
  date.className = "clock-date";

  const pomo = document.createElement("div");
  pomo.className = "pomo";
  const mode = document.createElement("div");
  mode.className = "pomo-mode";
  const ring = document.createElement("div");
  ring.className = "pomo-ring";
  const pomoTime = document.createElement("div");
  pomoTime.className = "pomo-time";
  ring.append(pomoTime);
  const row = document.createElement("div");
  row.className = "pomo-row";
  const startBtn = document.createElement("button");
  startBtn.className = "pill";
  startBtn.textContent = "start";
  const resetBtn = document.createElement("button");
  resetBtn.className = "pill ghost";
  resetBtn.textContent = "reset";
  for (const b of [startBtn, resetBtn]) {
    b.addEventListener("pointerdown", (e) => e.stopPropagation());
  }
  row.append(startBtn, resetBtn);
  const cycles = document.createElement("div");
  cycles.className = "pomo-cycles";
  pomo.append(mode, ring, row, cycles);
  wrap.append(time, date, pomo);
  root.append(wrap);

  // scale content to fit: shrinking the window zooms out instead of hiding widgets
  const fitClock = () => {
    const z = Math.min(1, window.innerWidth / 250, window.innerHeight / 330);
    wrap.style.setProperty("zoom", z > 0 ? String(z) : "1");
  };
  window.addEventListener("resize", fitClock);
  fitClock();

  const tickClock = () => {
    const now = new Date();
    time.textContent = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    date.textContent = now.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
  };
  tickClock();
  window.setInterval(tickClock, 5000);

  let phase: "focus" | "break" = "focus";
  let focusS = DEFAULT_FOCUS_S;
  let breakS = DEFAULT_BREAK_S;
  let left = focusS;
  let recRef: WidgetRecord | undefined;
  let running = false;
  let done = 0;
  let timer: number | undefined;

  const render = () => {
    mode.textContent = phase === "focus" ? "focus" : "break";
    wrap.dataset.phase = phase;
    pomoTime.textContent = fmt(left);
    const total = phase === "focus" ? focusS : breakS;
    const pct = 100 - Math.round((left / total) * 100);
    ring.style.setProperty("--pct", `${pct}%`);
    startBtn.textContent = running ? "pause" : "start";
    cycles.textContent = done === 0 ? "no rounds yet" : `${done} round${done === 1 ? "" : "s"} done`;
  };

  const step = () => {
    left -= 1;
    if (left <= 0) {
      if (phase === "focus") {
        done += 1;
        phase = "break";
        left = breakS;
      } else {
        phase = "focus";
        left = focusS;
      }
    }
    render();
  };

  startBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    running = !running;
    window.clearInterval(timer);
    if (running) timer = window.setInterval(step, 1000);
    render();
  });
  resetBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    running = false;
    window.clearInterval(timer);
    phase = "focus";
    left = focusS;
    render();
  });
  render();

  // tap the timer to edit focus/break lengths (minutes); persisted per widget
  const editRow = document.createElement("div");
  editRow.className = "pomo-edit";
  editRow.style.display = "none";
  const focusInput = document.createElement("input");
  focusInput.className = "pomo-num";
  focusInput.type = "number";
  focusInput.min = "1";
  focusInput.max = "180";
  focusInput.title = "Focus minutes";
  const breakInput = document.createElement("input");
  breakInput.className = "pomo-num";
  breakInput.type = "number";
  breakInput.min = "1";
  breakInput.max = "180";
  breakInput.title = "Break minutes";
  const saveEdit = document.createElement("button");
  saveEdit.className = "pill small";
  saveEdit.textContent = "set";
  editRow.append(focusInput, breakInput, saveEdit);
  pomo.insertBefore(editRow, cycles);
  const openEdit = () => {
    if (running) return;
    focusInput.value = String(Math.round(focusS / 60));
    breakInput.value = String(Math.round(breakS / 60));
    row.style.display = "none";
    editRow.style.display = "flex";
    focusInput.focus();
    focusInput.select();
  };
  const closeEdit = (save: boolean) => {
    if (save) {
      const fm = Math.min(180, Math.max(1, Math.round(Number(focusInput.value) || 0)));
      const bm = Math.min(180, Math.max(1, Math.round(Number(breakInput.value) || 0)));
      if (fm > 0) focusS = fm * 60;
      if (bm > 0) breakS = bm * 60;
      running = false;
      window.clearInterval(timer);
      phase = "focus";
      left = focusS;
      if (recRef) {
        recRef.data["focus_s"] = focusS;
        recRef.data["break_s"] = breakS;
        void saveRecord(recRef);
      }
    }
    editRow.style.display = "none";
    row.style.display = "flex";
    render();
  };
  pomoTime.style.cursor = "pointer";
  pomoTime.title = "Click to edit timer lengths";
  pomoTime.addEventListener("click", (e) => {
    e.stopPropagation();
    openEdit();
  });
  saveEdit.addEventListener("click", (e) => {
    e.stopPropagation();
    closeEdit(true);
  });
  for (const inp of [focusInput, breakInput]) {
    inp.addEventListener("pointerdown", (e) => e.stopPropagation());
    inp.addEventListener("keydown", (e) => {
      if (e.key === "Enter") closeEdit(true);
      else if (e.key === "Escape") closeEdit(false);
    });
  }

  void (async () => {
    const rec = await loadRecord(id);
    if (!rec) return;
    wrap.prepend(makeBar("clock", () => void removeSelf(rec), id));
    addPinMenu(wrap, () => rec);
    recRef = rec;
    // restore saved timer lengths (seconds, clamped to 1m..3h)
    const fs = typeof rec.data["focus_s"] === "number"
      ? Math.min(10800, Math.max(60, Math.round(rec.data["focus_s"] as number)))
      : 0;
    const bs = typeof rec.data["break_s"] === "number"
      ? Math.min(10800, Math.max(60, Math.round(rec.data["break_s"] as number)))
      : 0;
    if (fs > 0) {
      focusS = fs;
      left = fs;
      phase = "focus";
    }
    if (bs > 0) breakS = bs;
    render();
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
    addResizeHandle(wrap, rec, 200, 260);
    trackPosition(rec);
  })();
}

export const clockPlugin: FloatyPlugin = {
  kind: "clock",
  mount: mountClock,
  describe: () => undefined,
};
