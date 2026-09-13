import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  addPinMenu,
  addResizeHandle,
  appWin,
  currentSettings,
  enableOverlayDrag,
  loadRecord,
  removeSelf,
  saveRecord,
  trackPosition,
  watchPluginEnabled,
  watchSettings,
  type WidgetRecord,
} from "./lib";
import type { FloatyPlugin, PluginRecord, PluginSettingsContext } from "./plugin";

interface SysmonSample {
  cpu: number;
  gpu: number;
  ram: number;
  mem_used_mb: number;
  mem_total_mb: number;
}

const METRICS = ["gpu", "cpu", "ram"] as const;
type Metric = (typeof METRICS)[number];

/** Sample period the widget asks the backend for, honouring the setting. */
const interval = (): number => {
  const v = currentSettings().sysmon_interval;
  return typeof v === "number" && v >= 250 ? Math.min(10_000, Math.round(v)) : 1000;
};

const gb = (mb: number): string => (mb / 1024).toFixed(1);

/**
 * Live CPU / GPU / RAM monitor.
 *
 * This widget is the one that must never cost GPU: the backend pushes one
 * sample per interval (1 Hz by default) and the widget paints exactly one
 * frame per sample — there is no animation loop, no rAF, and the canvas is
 * only touched when a sample arrives or the widget is resized. On a desktop
 * full of widgets that makes it the cheapest thing on screen while still
 * showing what everything else is costing.
 */
export function mountSysmon(root: HTMLElement, id: string): void {
  watchSettings();
  watchPluginEnabled("sysmon");

  const wrap = document.createElement("div");
  wrap.className = "sysmon-wrap";
  wrap.innerHTML = `
    <div class="sysmon-top">
      <span class="sysmon-title">system</span>
      <button class="icon-btn sysmon-x" title="Remove">\u00d7</button>
    </div>
    <div class="sysmon-metrics">
      <div class="sysmon-metric" data-metric="cpu">
        <span class="sysmon-key">cpu</span>
        <span class="sysmon-val">--</span>
        <span class="sysmon-track"><i></i></span>
      </div>
      <div class="sysmon-metric" data-metric="gpu">
        <span class="sysmon-key">gpu</span>
        <span class="sysmon-val">--</span>
        <span class="sysmon-track"><i></i></span>
      </div>
      <div class="sysmon-metric" data-metric="ram">
        <span class="sysmon-key">ram</span>
        <span class="sysmon-val">--</span>
        <span class="sysmon-track"><i></i></span>
      </div>
    </div>
    <canvas class="sysmon-canvas"></canvas>
    <div class="sysmon-foot">graph: gpu &middot; click to cycle</div>
  `;
  root.append(wrap);

  const canvas = wrap.querySelector<HTMLCanvasElement>(".sysmon-canvas")!;
  const ctx = canvas.getContext("2d");
  const xBtn = wrap.querySelector<HTMLButtonElement>(".sysmon-x")!;
  xBtn.addEventListener("pointerdown", (e) => e.stopPropagation());

  const rows: Record<Metric, { val: HTMLElement; bar: HTMLElement }> = {
    cpu: {
      val: wrap.querySelector<HTMLElement>('[data-metric="cpu"] .sysmon-val')!,
      bar: wrap.querySelector<HTMLElement>('[data-metric="cpu"] .sysmon-track i')!,
    },
    gpu: {
      val: wrap.querySelector<HTMLElement>('[data-metric="gpu"] .sysmon-val')!,
      bar: wrap.querySelector<HTMLElement>('[data-metric="gpu"] .sysmon-track i')!,
    },
    ram: {
      val: wrap.querySelector<HTMLElement>('[data-metric="ram"] .sysmon-val')!,
      bar: wrap.querySelector<HTMLElement>('[data-metric="ram"] .sysmon-track i')!,
    },
  };
  const foot = wrap.querySelector<HTMLElement>(".sysmon-foot")!;

  let rec: WidgetRecord | undefined;
  let graph: Metric = "gpu";
  const history: Record<Metric, number[]> = { cpu: [], gpu: [], ram: [] };
  let latest: SysmonSample | undefined;
  /** draws since the last resize; resizing invalidates the canvas */
  let drawTimer: number | undefined;
  let peak: Record<Metric, number> = { cpu: 0, gpu: 0, ram: 0 };

  const resizeCanvas = (): void => {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
      return;
    }
  };

  const COLORS: Record<Metric, [string, string]> = {
    gpu: ["rgba(139, 123, 255, 0.55)", "rgba(167, 155, 255, 1)"],
    cpu: ["rgba(94, 234, 212, 0.45)", "rgba(94, 234, 212, 1)"],
    ram: ["rgba(251, 191, 36, 0.40)", "rgba(252, 211, 77, 1)"],
  };

  /** One frame per sample: grid, filled history line, current point. */
  const draw = (): void => {
    if (!ctx) return;
    const w = canvas.width;
    const h = canvas.height;
    if (w < 2 || h < 2) return;
    ctx.clearRect(0, 0, w, h);

    const [fill, line] = COLORS[graph];
    ctx.lineWidth = Math.max(1, h * 0.035);
    ctx.strokeStyle = "rgba(255, 255, 255, 0.09)";
    for (const frac of [0.25, 0.5, 0.75]) {
      const y = Math.round(h * frac) + 0.5;
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(w, y);
      ctx.stroke();
    }

    const data = history[graph];
    if (data.length === 0) return;
    const n = data.length;
    const px = (i: number): number => (n === 1 ? w / 2 : (i / (n - 1)) * w);
    const py = (v: number): number => h - (Math.max(0, Math.min(100, v)) / 100) * h;

    if (n > 1) {
      ctx.beginPath();
      ctx.moveTo(px(0), py(data[0]));
      for (let i = 1; i < n; i++) ctx.lineTo(px(i), py(data[i]));
      ctx.lineTo(w, h);
      ctx.lineTo(px(0), h);
      ctx.closePath();
      ctx.fillStyle = fill;
      ctx.fill();

      ctx.beginPath();
      ctx.moveTo(px(0), py(data[0]));
      for (let i = 1; i < n; i++) ctx.lineTo(px(i), py(data[i]));
      ctx.strokeStyle = line;
      ctx.lineJoin = "round";
      ctx.stroke();
    }

    ctx.beginPath();
    ctx.arc(px(n - 1), py(data[n - 1]), Math.max(1.5, h * 0.045), 0, Math.PI * 2);
    ctx.fillStyle = line;
    ctx.fill();
  };

  const paint = (): void => {
    if (latest) {
      const s = latest;
      const values: Record<Metric, number> = { cpu: s.cpu, gpu: s.gpu, ram: s.ram };
      for (const m of METRICS) {
        const v = Math.max(0, Math.min(100, values[m]));
        rows[m].val.textContent = m === "ram"
          ? `${v.toFixed(0)}%  ${gb(s.mem_used_mb)}/${gb(s.mem_total_mb)}g`
          : `${v.toFixed(0)}%`;
        rows[m].bar.style.width = `${v.toFixed(1)}%`;
        // colour the bar with its own metric colour so the graph legend is obvious
        rows[m].bar.style.background = COLORS[m][1];
      }
      wrap.dataset.graph = graph;
      const p = peak[graph];
      foot.textContent = `graph: ${graph}  peak ${p.toFixed(0)}%  ·  click to cycle`;
    }
    draw();
  };

  const onSample = (payload: SysmonSample): void => {
    if (!payload) return;
    latest = payload;
    for (const m of METRICS) {
      const v = Math.max(0, Math.min(100, payload[m]));
      const arr = history[m];
      arr.push(v);
      while (arr.length > 60) arr.shift();
      if (v > peak[m]) peak[m] = v;
    }
    paint();
  };

  const saveGraph = (): void => {
    if (!rec) return;
    rec.data["graph"] = graph;
    void saveRecord(rec).catch(() => undefined);
  };

  wrap.addEventListener("click", (e) => {
    e.stopPropagation();
    graph = METRICS[(METRICS.indexOf(graph) + 1) % METRICS.length];
    saveGraph();
    paint();
  });
  wrap.addEventListener("pointerdown", (e) => {
    if (e.button === 0) e.stopPropagation();
  });
  wrap.addEventListener("contextmenu", (e) => e.preventDefault());

  const scheduleRedraw = (): void => {
    window.clearTimeout(drawTimer);
    drawTimer = window.setTimeout(() => {
      resizeCanvas();
      paint();
    }, 120);
  };
  window.addEventListener("resize", scheduleRedraw);

  void listen<SysmonSample>("floaty-sysmon", (e) => onSample(e.payload)).catch(() => undefined);

  void (async () => {
    rec = await loadRecord(id);
    if (!rec) {
      wrap.remove();
      return;
    }
    const saved = rec.data["graph"];
    if (typeof saved === "string" && (METRICS as readonly string[]).includes(saved)) {
      graph = saved as Metric;
    }
    xBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      void removeSelf(rec!);
    });
    addResizeHandle(wrap, rec, 180, 110);
    // no title bar: the whole surface drags, and a click still cycles the graph
    enableOverlayDrag(wrap, id);
    trackPosition(rec);
    addPinMenu(wrap, () => rec);
    window.addEventListener("beforeunload", () => {
      void invoke("floaty_sysmon_stop").catch(() => undefined);
    });

    await invoke("floaty_sysmon_start", { intervalMs: interval() }).catch(() => undefined);
    // if no sample lands and the backend says it is not sampling, say so rather
    // than showing dashes forever (GPU performance counters can be unavailable)
    window.setTimeout(() => {
      if (latest) return;
      void invoke<boolean>("floaty_sysmon_status")
        .then((running) => {
          if (!running) foot.textContent = "sampler unavailable on this system";
        })
        .catch(() => undefined);
    }, Math.max(1500, interval() * 3));
    listen("floaty-settings-changed", () => {
      void invoke("floaty_sysmon_set_interval", { ms: interval() }).catch(() => undefined);
    }).catch(() => undefined);

    scheduleRedraw();
    void appWin.show().catch(() => undefined);
  })();
}

export const sysmonPlugin: FloatyPlugin = {
  kind: "sysmon",
  mount: mountSysmon,
  describe: (rec: PluginRecord) => {
    const g = rec.data["graph"];
    return typeof g === "string" && g ? `${g} graph` : "system monitor";
  },
  renderSettings: (card: HTMLElement, ctx: PluginSettingsContext) => {
    const r = ctx.getSharedRow("sysmon_interval");
    if (r) card.append(r);
  },
};
