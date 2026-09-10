import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import * as PIXI from "pixi.js";
import { Live2DModel, MotionPriority } from "pixi-live2d-display/cubism2";
import {
  addPinMenu,
  appWin,
  loadRecord,
  logicalPos,
  removeSelf,
  setLogicalPos,
  trackPosition,
  watchPluginEnabled,
} from "./lib";

type L2DModel = Awaited<ReturnType<typeof Live2DModel.from>>;
const BUNDLED_MODEL = "/live2d/hijiki/hijiki.model.json";

async function ensureCore(): Promise<void> {
  if ((window as unknown as { Live2D?: unknown }).Live2D) return;
  await new Promise<void>((resolve, reject) => {
    const s = document.createElement("script");
    s.src = "/live2d.min.js";
    s.onload = () => resolve();
    s.onerror = () => reject(new Error("live2d core failed to load"));
    document.head.append(s);
  });
}

/** Bundled web path stays as-is; user-library files go through the asset protocol. */
function resolveModelUrl(model: string): string {
  if (!model) return BUNDLED_MODEL;
  return model.startsWith("/") ? model : convertFileSrc(model);
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

  const fitModel = (model: L2DModel): void => {
    model.scale.set(1);
    const s = Math.min(window.innerWidth / model.width, window.innerHeight / model.height);
    model.scale.set(Number.isFinite(s) && s > 0 ? s * 0.98 : 1);
    model.anchor.set(0.5, 0.5);
    model.position.set(window.innerWidth / 2, window.innerHeight / 2);
  };

  const showModel = async (url: string): Promise<void> => {
    if (!pixiApp) return;
    if (currentModel) {
      try {
        pixiApp.stage.removeChild(currentModel);
        currentModel.destroy();
      } catch {
        /* ignore */
      }
      currentModel = undefined;
    }
    const model = await Live2DModel.from(url, { autoInteract: true });
    currentModel = model;
    pixiApp.stage.addChild(model);
    fitModel(model);
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
      void removeSelf(rec);
    });
    addPinMenu(wrap, () => rec);
    trackPosition(rec);

    try {
      await ensureCore();
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
      // idle loop filler; IDLE priority never interrupts tap motions
      window.setInterval(() => {
        if (!currentModel) return;
        try {
          void currentModel.motion("idle", undefined, MotionPriority.IDLE);
        } catch {
          /* model idles on its own */
        }
      }, 20000);
    } catch {
      failedBox();
      return;
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
