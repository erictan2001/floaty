import { PhysicalSize } from "@tauri-apps/api/window";
import { addResizeHandle, appWin, loadRecord, makeBar, removeSelf, trackPosition } from "./lib";

const FOCUS_S = 25 * 60;
const BREAK_S = 5 * 60;

function fmt(total: number): string {
  const m = Math.floor(total / 60).toString().padStart(2, "0");
  const s = (total % 60).toString().padStart(2, "0");
  return `${m}:${s}`;
}

export function mountClock(root: HTMLElement, id: string): void {
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

  const tickClock = () => {
    const now = new Date();
    time.textContent = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    date.textContent = now.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
  };
  tickClock();
  window.setInterval(tickClock, 5000);

  let phase: "focus" | "break" = "focus";
  let left = FOCUS_S;
  let running = false;
  let done = 0;
  let timer: number | undefined;

  const render = () => {
    mode.textContent = phase === "focus" ? "focus" : "break";
    wrap.dataset.phase = phase;
    pomoTime.textContent = fmt(left);
    const total = phase === "focus" ? FOCUS_S : BREAK_S;
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
        left = BREAK_S;
      } else {
        phase = "focus";
        left = FOCUS_S;
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
    left = FOCUS_S;
    render();
  });
  render();

  void (async () => {
    const rec = await loadRecord(id);
    if (!rec) return;
    wrap.prepend(makeBar("floaty clock", () => void removeSelf(rec)));
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
    addResizeHandle(wrap, rec, 200, 260);
    trackPosition(rec);
  })();
}
