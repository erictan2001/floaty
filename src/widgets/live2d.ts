import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import * as PIXI from "pixi.js";
import "@pixi/unsafe-eval";
import { Live2DModel, MotionPriority } from "pixi-live2d-display";

// Expose PIXI and register ticker so pixi-live2d-display can autoUpdate models
Live2DModel.registerTicker(PIXI.Ticker);
(window as unknown as { PIXI?: unknown }).PIXI = PIXI;
import {
  addPinMenu,
  appWin,
  ensureLive2DCore,
  loadRecord,
  logicalPos,
  removeSelf,
  setLogicalPos,
  trackPosition,
  watchPluginEnabled,
} from "./lib";

type L2DModel = Awaited<ReturnType<typeof Live2DModel.from>>;
const BUNDLED_MODEL = "/live2d/hijiki/hijiki.model.json";

/** Bundled web path stays as-is; user-library files go through the asset protocol.
 * Preserving forward slashes ensures pixi-live2d-display correctly resolves relative paths
 * (textures, motions, expressions, moc) to the model's folder rather than the host root.
 */
function resolveModelUrl(model: string): string {
  if (!model) return BUNDLED_MODEL;
  if (model.startsWith("/")) return model;
  const normalized = model.replace(/\\/g, "/");
  const parts = normalized.split("/").map((part, idx) => {
    if (idx === 0 && /^[a-zA-Z]:$/.test(part)) return part;
    return encodeURIComponent(part);
  });
  return `http://asset.localhost/${parts.join("/")}`;
}

export function mountLive2D(root: HTMLElement, id: string): void {
  watchPluginEnabled("live2d");
  const wrap = document.createElement("div");
  wrap.className = "live2d-wrap";
  const canvas = document.createElement("canvas");
  wrap.append(canvas);
  const x = document.createElement("button");
  x.className = "icon-btn live2d-x";
  x.title = "Remove";
  x.textContent = "×";
  x.addEventListener("pointerdown", (e) => e.stopPropagation());
  wrap.append(x);
  root.append(wrap);

  let dragging = false;
  let pixiApp: PIXI.Application | undefined;
  let currentModel: L2DModel | undefined;
  let trackingTimer: number | undefined;

  const fitModel = (model: L2DModel): void => {
    model.scale.set(1);
    const mw = model.width || (model as unknown as { internalModel?: { width?: number } }).internalModel?.width || 300;
    const mh = model.height || (model as unknown as { internalModel?: { height?: number } }).internalModel?.height || 400;
    const s = Math.min(window.innerWidth / mw, window.innerHeight / mh);
    model.scale.set(Number.isFinite(s) && s > 0 ? s * 0.98 : 1);
    model.anchor.set(0.5, 0.5);
    model.position.set(window.innerWidth / 2, window.innerHeight / 2);
  };

  /** Focus the model toward a point in logical window pixels (can be negative or outside the window) */
  const focusModel = (model: L2DModel, relX: number, relY: number): void => {
    const internal = (
      model as unknown as {
        internalModel?: {
          focusController?: { focus: (x: number, y: number) => void };
          originalWidth?: number;
          originalHeight?: number;
        };
      }
    ).internalModel;
    if (!internal?.focusController) {
      model.focus(relX, relY);
      return;
    }
    const p = new PIXI.Point(relX, relY);
    model.toModelPosition(p, p, true);
    const w = internal.originalWidth || 300;
    const h = internal.originalHeight || 400;
    const tx = (p.x / w) * 2 - 1;
    const ty = (p.y / h) * 2 - 1;
    const dist = Math.hypot(tx, ty);
    if (dist < 0.001) {
      internal.focusController.focus(0, 0);
    } else {
      const factor = Math.min(1, dist);
      const radian = Math.atan2(ty, tx);
      internal.focusController.focus(Math.cos(radian) * factor, -Math.sin(radian) * factor);
    }
  };

  const showModel = async (url: string): Promise<void> => {
    if (!pixiApp) return;
    invoke("floaty_log", { msg: `[live2d/${id}] showModel: ${url}` }).catch(() => undefined);
    if (currentModel) {
      try {
        pixiApp.stage.removeChild(currentModel);
        currentModel.destroy();
      } catch {
        /* ignore */
      }
      currentModel = undefined;
    }
    try {
      const model = await Live2DModel.from(url, { autoInteract: true });
      currentModel = model;
      pixiApp.stage.addChild(model);
      fitModel(model);
      wrap.querySelector(".live2d-failed")?.remove();
      invoke("floaty_log", { msg: `[live2d/${id}] model loaded successfully: ${url}` }).catch(() => undefined);
    } catch (err) {
      invoke("floaty_log", { msg: `[live2d/${id}] model load failed: ${String(err)}` }).catch(() => undefined);
      failedBox();
      throw err;
    }
  };

  const failedBox = (): void => {
    if (wrap.querySelector(".live2d-failed")) return;
    const f = document.createElement("div");
    f.className = "live2d-failed";
    f.textContent = "live2d failed to load";
    wrap.append(f);
  };

  void (async () => {
    const rec = await loadRecord(id);
    if (!rec) {
      wrap.remove();
      return;
    }
    x.addEventListener("click", (e) => {
      e.stopPropagation();
      if (trackingTimer) window.clearInterval(trackingTimer);
      void removeSelf(rec);
    });
    window.addEventListener("beforeunload", () => {
      if (trackingTimer) window.clearInterval(trackingTimer);
    });
    addPinMenu(wrap, () => rec);
    trackPosition(rec);

    try {
      await ensureLive2DCore();
      pixiApp = new PIXI.Application({
        view: canvas,
        backgroundAlpha: 0,
        antialias: true,
        resolution: window.devicePixelRatio || 1,
        autoDensity: true,
        resizeTo: window,
      });
      const m = typeof rec.data["model"] === "string" ? (rec.data["model"] as string) : "";
      await showModel(resolveModelUrl(m));
      window.addEventListener("resize", () => {
        if (currentModel) fitModel(currentModel);
      });
      // Follow user mouse movement across the entire desktop (including outside the window)
      let lastRelX = -99999;
      let lastRelY = -99999;

      window.addEventListener("pointermove", (e) => {
        if (!currentModel || dragging) return;
        lastRelX = e.clientX;
        lastRelY = e.clientY;
        focusModel(currentModel, e.clientX, e.clientY);
      });

      const pollDesktopCursor = async (): Promise<void> => {
        if (document.hidden || dragging || !currentModel) return;
        try {
          const pos = await invoke<{ rel_x: number; rel_y: number } | null>(
            "floaty_window_cursor_pos",
            { label: appWin.label },
          );
          if (!pos || !currentModel || dragging) return;
          if (Math.abs(pos.rel_x - lastRelX) < 1.5 && Math.abs(pos.rel_y - lastRelY) < 1.5) {
            return;
          }
          lastRelX = pos.rel_x;
          lastRelY = pos.rel_y;
          focusModel(currentModel, pos.rel_x, pos.rel_y);
        } catch {
          /* ignore */
        }
      };
      trackingTimer = window.setInterval(() => {
        void pollDesktopCursor();
      }, 35);

      // idle loop filler; IDLE priority never interrupts tap motions
      window.setInterval(() => {
        if (!currentModel) return;
        try {
          void currentModel.motion("idle", undefined, MotionPriority.IDLE);
        } catch {
          /* model idles on its own */
        }
      }, 20000);
    } catch (err) {
      invoke("floaty_log", { msg: `[live2d/${id}] init failed: ${String(err)}` }).catch(() => undefined);
      failedBox();
    }
    await listen<string>("floaty-live2d-changed", (e) => {
      if (e.payload !== id) return;
      void (async () => {
        const r = await loadRecord(id);
        if (!r) return;
        const m = typeof r.data["model"] === "string" ? (r.data["model"] as string) : "";
        await showModel(resolveModelUrl(m)).catch(() => undefined);
      })();
    }).catch(() => undefined);
    void appWin.show().catch(() => undefined);
  })();

  // manual pinned drag (no gravity); taps pass through to the model
  wrap.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || dragging) return;
    const t = e.target as HTMLElement;
    if (t.closest("button")) return;
    e.stopPropagation();
    dragging = true;
    try {
      wrap.setPointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
    void (async () => {
      let bx = 0;
      let by = 0;
      try {
        const p = await logicalPos();
        bx = p.x;
        by = p.y;
      } catch {
        dragging = false;
        return;
      }
      const sx = e.screenX;
      const sy = e.screenY;
      const onMove = (ev: PointerEvent) => {
        void setLogicalPos(
          Math.round(bx + (ev.screenX - sx)),
          Math.round(by + (ev.screenY - sy)),
        ).catch(() => undefined);
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
        dragging = false;
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onUp);
    })();
  });
  wrap.addEventListener("contextmenu", (e) => e.preventDefault());
}
