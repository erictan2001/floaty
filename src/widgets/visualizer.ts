import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  addPinMenu,
  addResizeHandle,
  appWin,
  currentSettings,
  enableOverlayDrag,
  loadRecord,
  onSettings,
  presenceIsQuiet,
  removeSelf,
  saveRecord,
  trackPosition,
  watchPluginEnabled,
  watchSettings,
  type WidgetRecord,
} from "./lib";
import type { FloatyPlugin, PluginRecord, PluginSettingsContext } from "./plugin";

interface AudioLevels {
  bands: number[];
  level: number;
  silent: boolean;
}

const MODES = ["bars", "wave", "dots"] as const;
type VizMode = (typeof MODES)[number];

/**
 * Spectrum for system audio. The backend (WASAPI loopback + FFT) pushes band
 * magnitudes; this widget only draws them.
 *
 * Drawing is deliberately stingy: it lives inside the full-desktop overlay, so
 * every painted frame is a full-screen composite. The loop runs at the
 * configured `viz_fps` while audio plays and stops completely once the backend
 * reports silence — a desktop full of widgets should not pay for a visualizer
 * nobody is feeding.
 */
export function mountVisualizer(root: HTMLElement, id: string): void {
  watchSettings();
  watchPluginEnabled("visualizer");

  const wrap = document.createElement("div");
  wrap.className = "viz-wrap";
  wrap.innerHTML = `
    <canvas class="viz-canvas"></canvas>
    <button class="icon-btn viz-x" title="Remove">\u00d7</button>
    <div class="viz-hint">click: bars / wave / dots</div>
  `;
  root.append(wrap);
  const canvas = wrap.querySelector<HTMLCanvasElement>(".viz-canvas")!;
  const ctx = canvas.getContext("2d");
  const x = wrap.querySelector<HTMLButtonElement>(".viz-x")!;
  x.addEventListener("pointerdown", (e) => e.stopPropagation());

  let rec: WidgetRecord | undefined;
  let mode: VizMode = "bars";
  let bands: number[] = new Array(28).fill(0); // smoothed, what we draw
  let target: number[] = new Array(28).fill(0); // latest from the backend
  let level = 0;
  let lastEvent = 0;
  let silent = true;
  let rafId: number | undefined;
  let lastDraw = 0;

  const fps = (): number => {
    const v = currentSettings().viz_fps;
    return typeof v === "number" && v >= 5 ? Math.min(60, v) : 30;
  };
  const gain = (): number => {
    const v = currentSettings().viz_gain;
    return typeof v === "number" && v > 0 ? Math.min(4, v) : 1;
  };

  const resizeCanvas = (): void => {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = Math.max(1, Math.round(wrap.clientWidth * dpr));
    const h = Math.max(1, Math.round(wrap.clientHeight * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
  };

  const drawBars = (w: number, h: number, usable: number): void => {
    if (!ctx) return;
    const n = bands.length;
    const gap = Math.max(1, Math.round(w / n / 5));
    const bw = Math.max(1, w / n - gap);
    // roundRect is Chromium 99+; fall back to plain rects rather than throwing
    const rounded = typeof (ctx as CanvasRenderingContext2D & { roundRect?: unknown }).roundRect === "function";
    for (let i = 0; i < n; i++) {
      const v = Math.max(0.012, bands[i]);
      const bh = Math.max(2, usable * v);
      const px = i * (bw + gap) + gap / 2;
      const py = h / 2 - bh / 2;
      ctx.globalAlpha = 0.85;
      if (rounded) {
        ctx.beginPath();
        const r = Math.min(bw / 2, 6);
        ctx.roundRect(px, py, bw, bh, r);
        ctx.fill();
      } else {
        ctx.fillRect(px, py, bw, bh);
      }
    }
    ctx.globalAlpha = 1;
  };

  const drawWave = (w: number, h: number, usable: number): void => {
    if (!ctx) return;
    const n = bands.length;
    ctx.lineWidth = Math.max(2, h * 0.045);
    ctx.lineJoin = "round";
    ctx.beginPath();
    for (let i = 0; i < n; i++) {
      const px = (i / (n - 1)) * w;
      const py = h / 2 - (bands[i] - 0.5) * usable * 1.4;
      if (i === 0) ctx.moveTo(px, py);
      else ctx.lineTo(px, py);
    }
    ctx.globalAlpha = 0.9;
    ctx.stroke();
    // mirrored ghost for the classic look
    ctx.globalAlpha = 0.35;
    ctx.beginPath();
    for (let i = 0; i < n; i++) {
      const px = (i / (n - 1)) * w;
      const py = h / 2 + (bands[i] - 0.5) * usable * 1.4;
      if (i === 0) ctx.moveTo(px, py);
      else ctx.lineTo(px, py);
    }
    ctx.stroke();
    ctx.globalAlpha = 1;
  };

  const drawDots = (w: number, h: number): void => {
    if (!ctx) return;
    const n = bands.length;
    const step = w / n;
    for (let i = 0; i < n; i++) {
      const v = bands[i];
      const r = Math.max(1.5, (step * 0.36) * (0.4 + v * 0.9));
      ctx.globalAlpha = 0.35 + v * 0.65;
      ctx.beginPath();
      ctx.arc(i * step + step / 2, h / 2, r, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
  };

  /** Bars, a mirrored waveform or a row of glowing dots. */
  const draw = (): void => {
    if (!ctx) return;
    const w = canvas.width;
    const h = canvas.height;
    ctx.clearRect(0, 0, w, h);
    const pad = Math.round(h * 0.12);
    const usable = h - pad * 2;

    const grad = ctx.createLinearGradient(0, h, 0, 0);
    grad.addColorStop(0, "rgba(108, 92, 231, 0.95)");
    grad.addColorStop(0.55, "rgba(139, 123, 255, 0.95)");
    grad.addColorStop(1, "rgba(94, 234, 212, 0.95)");

    ctx.fillStyle = grad;
    ctx.strokeStyle = grad;

    if (mode === "bars") {
      drawBars(w, h, usable);
    } else if (mode === "wave") {
      drawWave(w, h, usable);
    } else {
      drawDots(w, h);
    }
  };

  const frame = (now: number): void => {
    rafId = undefined;
    const interval = 1000 / fps();
    if (now - lastDraw < interval) {
      schedule();
      return;
    }
    lastDraw = now;
    // lerp toward the backend values so 30fps updates still look fluid
    const g = gain();
    for (let i = 0; i < bands.length; i++) {
      const t = Math.min(1, (target[i] ?? 0) * g);
      bands[i] += (t - bands[i]) * 0.45;
    }
    level += ((silent ? 0 : level) - level) * 0.2;
    wrap.style.setProperty("--viz-level", level.toFixed(3));
    draw();
    // stop when the audio has been quiet for a while: no more frames at all
    const stale = performance.now() - lastEvent > 600;
    if (silent && stale) {
      rafId = undefined;
      return;
    }
    schedule();
  };

  const schedule = (): void => {
    if (rafId === undefined && !document.hidden) {
      rafId = requestAnimationFrame(frame);
    }
  };

  const saveMode = (): void => {
    if (!rec) return;
    rec.data["mode"] = mode;
    void saveRecord(rec).catch(() => undefined);
  };

  wrap.addEventListener("click", (e) => {
    e.stopPropagation();
    mode = MODES[(MODES.indexOf(mode) + 1) % MODES.length];
    saveMode();
  });
  wrap.addEventListener("pointerdown", (e) => {
    if (e.button === 0) e.stopPropagation();
  });
  wrap.addEventListener("contextmenu", (e) => e.preventDefault());

  window.addEventListener("resize", () => {
    resizeCanvas();
    lastDraw = 0;
    schedule();
  });
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) schedule();
  });

  void listen<AudioLevels>("floaty-audio-levels", (e) => {
    const p = e.payload;
    if (!p || !Array.isArray(p.bands)) return;
    target = p.bands;
    if (bands.length !== p.bands.length) bands = new Array(p.bands.length).fill(0);
    silent = !!p.silent;
    lastEvent = performance.now();
    schedule();
  }).catch(() => undefined);

  void (async () => {
    rec = await loadRecord(id);
    if (!rec) {
      wrap.remove();
      return;
    }
    const saved = rec.data["mode"];
    if (typeof saved === "string" && (MODES as readonly string[]).includes(saved)) {
      mode = saved as VizMode;
    }
    x.addEventListener("click", (e) => {
      e.stopPropagation();
      void removeSelf(rec!);
    });
    addResizeHandle(wrap, rec, 160, 80);
    // no title bar: the whole surface drags, and a click still cycles the mode
    enableOverlayDrag(wrap, id);
    trackPosition(rec);
    window.addEventListener("beforeunload", () => {
      void invoke("floaty_audio_stop").catch(() => undefined);
    });
    addPinMenu(wrap, () => rec);

    // Capture is what costs while nothing is happening, so quiet stops it — and this is
    // also the path back: Rust pauses the capture when the machine goes idle and the
    // widget asks for it again when input returns (a pause, not a switch that stays off).
    const syncCapture = () => {
      if (presenceIsQuiet()) {
        void invoke("floaty_audio_stop").catch(() => undefined);
      } else {
        void invoke("floaty_audio_start", { fps: fps() }).catch(() => undefined);
      }
    };
    window.addEventListener("floaty-presence", syncCapture);
    if (!presenceIsQuiet()) {
      await invoke("floaty_audio_start", { fps: fps() }).catch(() => undefined);
    }
    // if nothing arrives and capture is not running, say why instead of looking
    // like a broken flat line (loopback capture needs a default render device)
    window.setTimeout(() => {
      if (lastEvent) return;
      void invoke<boolean>("floaty_audio_status")
        .then((running) => {
          if (!running) wrap.querySelector(".viz-hint")!.textContent = "system audio unavailable";
        })
        .catch(() => undefined);
    }, 2500);
    onSettings(() => {
      void invoke("floaty_audio_set_fps", { fps: fps() }).catch(() => undefined);
    });

    resizeCanvas();
    void appWin.show().catch(() => undefined);
    // paint one idle frame so the widget is never a blank hole on screen
    draw();
  })();
}

export const visualizerPlugin: FloatyPlugin = {
  kind: "visualizer",
  mount: mountVisualizer,
  describe: (rec: PluginRecord) => {
    const m = rec.data["mode"];
    return typeof m === "string" && m ? `${m} visualizer` : "visualizer";
  },
  renderSettings: (card: HTMLElement, ctx: PluginSettingsContext) => {
    for (const k of ["viz_gain", "viz_fps"]) {
      const r = ctx.getSharedRow(k);
      if (r) card.append(r);
    }
  },
};
