import { invoke } from "@tauri-apps/api/core";
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
  debounce,
  ensureLive2DCore,
  loadRecord,
  logicalPos,
  removeSelf,
  saveRecord,
  setLogicalPos,
  trackPosition,
  watchPluginEnabled,
} from "./lib";

type L2DModel = Awaited<ReturnType<typeof Live2DModel.from>>;
const BUNDLED_MODEL = "/live2d/hijiki/hijiki.model.json";

interface AvailableMotion {
  group: string;
  index: number;
  descriptor: string;
  isIdle: boolean;
}

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

/** Inspect all motion groups and definitions in the model */
function getAvailableMotions(model: L2DModel): AvailableMotion[] {
  const result: AvailableMotion[] = [];
  const definitions = (
    model as unknown as {
      internalModel?: {
        motionManager?: {
          definitions?: Record<string, Array<{ File?: string; file?: string; name?: string }>>;
        };
      };
    }
  ).internalModel?.motionManager?.definitions;

  if (!definitions || typeof definitions !== "object") return result;

  for (const group of Object.keys(definitions)) {
    const list = definitions[group];
    if (!Array.isArray(list)) continue;
    for (let i = 0; i < list.length; i++) {
      const def = list[i];
      if (!def) continue;
      const fileOrName = (def.File || def.file || def.name || "").toLowerCase();
      const groupLower = group.toLowerCase();
      const fullDesc = `${groupLower} ${fileOrName}`;
      const isIdle = groupLower === "idle" || fileOrName.includes("idle");
      result.push({
        group,
        index: i,
        descriptor: fullDesc,
        isIdle,
      });
    }
  }
  return result;
}

/** Select and play the most fitting motion for the interaction target */
async function playInteractMotion(
  model: L2DModel,
  target: "head" | "body" | "special",
  priority: MotionPriority = MotionPriority.NORMAL,
): Promise<boolean> {
  const motions = getAvailableMotions(model);
  if (motions.length === 0) return false;

  let candidates: AvailableMotion[] = [];

  const headKeywords = ["touch_head", "tap_head", "flick_head", "head", "face", "hair", "ear"];
  const bodyKeywords = [
    "touch_body",
    "tap_body",
    "pinch_in",
    "pinch_out",
    "shake",
    "body",
    "chest",
    "belly",
    "bust",
    "main_",
    "tap",
    "touch",
  ];
  const specialKeywords = [
    "touch_special",
    "tap_special",
    "special",
    "wedding",
    "celebrate",
    "login",
    "home",
    "complete",
    "mission",
    "happy",
  ];

  const searchKeywords =
    target === "head" ? headKeywords : target === "special" ? specialKeywords : bodyKeywords;

  for (const kw of searchKeywords) {
    const matched = motions.filter((m) => m.descriptor.includes(kw));
    if (matched.length > 0) {
      candidates = matched;
      break;
    }
  }

  // Fallback 1: any non-idle motion
  if (candidates.length === 0) {
    candidates = motions.filter((m) => !m.isIdle);
  }

  // Fallback 2: any motion at all
  if (candidates.length === 0) {
    candidates = motions;
  }

  if (candidates.length === 0) return false;

  const chosen = candidates[Math.floor(Math.random() * candidates.length)];
  invoke("floaty_log", {
    msg: `[live2d] playing motion: group="${chosen.group}" index=${chosen.index} desc="${chosen.descriptor}" prio=${priority}`,
  }).catch(() => undefined);

  try {
    const ok = await model.motion(chosen.group, chosen.index, priority);
    if (!ok && priority === MotionPriority.NORMAL) {
      // Force playback so user interaction is always acknowledged even if another motion is finishing
      return await model.motion(chosen.group, chosen.index, MotionPriority.FORCE);
    }
    return ok;
  } catch {
    return false;
  }
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
  let modelScale = 1;
  let interactUntil = 0;

  const saveScale = debounce((mult: number) => {
    void (async () => {
      const r = await loadRecord(id);
      if (!r) return;
      r.data["scale"] = mult;
      await saveRecord(r);
    })();
  }, 400);

  const fitModel = (model: L2DModel, scaleMult = modelScale): void => {
    model.scale.set(1);
    const mw = model.width || (model as unknown as { internalModel?: { width?: number } }).internalModel?.width || 300;
    const mh = model.height || (model as unknown as { internalModel?: { height?: number } }).internalModel?.height || 400;
    const s = Math.min(window.innerWidth / mw, window.innerHeight / mh);
    const baseScale = Number.isFinite(s) && s > 0 ? s * 0.98 : 1;
    model.scale.set(baseScale * scaleMult);
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

  const handleModelInteract = (
    model: L2DModel,
    clientX: number,
    clientY: number,
    overrideTarget?: "head" | "body" | "special",
    priority: MotionPriority = MotionPriority.NORMAL,
  ): void => {
    // Focus head/eyes directly on tap location
    focusModel(model, clientX, clientY);

    // Trigger internal tap hit-test
    try {
      model.tap(clientX, clientY);
    } catch {
      /* ignore */
    }

    // Determine target region: check hit areas first, then model bounds geometry
    let target: "head" | "body" | "special" = overrideTarget || "body";
    if (!overrideTarget) {
      let hitAreas: string[] = [];
      try {
        hitAreas = model.hitTest(clientX, clientY) || [];
      } catch {
        /* ignore */
      }

      if (hitAreas.length > 0) {
        const joined = hitAreas.join(" ").toLowerCase();
        if (/head|face|hair|ear|hat|eye|mouth/.test(joined)) {
          target = "head";
        } else if (/special|star|acc/.test(joined)) {
          target = "special";
        } else {
          target = "body";
        }
      } else {
        const bounds = model.getBounds();
        const relY = bounds.height > 0 ? (clientY - bounds.y) / bounds.height : 0.5;
        if (relY < 0.38) {
          target = "head";
        } else if (relY > 0.78) {
          target = "special";
        } else {
          target = "body";
        }
      }
    }

    invoke("floaty_log", {
      msg: `[live2d/${id}] interact: target=${target} prio=${priority} pos=(${Math.round(clientX)},${Math.round(clientY)})`,
    }).catch(() => undefined);

    // Play corresponding motion
    void playInteractMotion(model, target, priority);

    // Trigger random expression change if expressions exist
    try {
      void model.expression().catch(() => undefined);
    } catch {
      /* ignore */
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
      fitModel(model, modelScale);
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
    if (typeof rec.data["scale"] === "number" && (rec.data["scale"] as number) > 0) {
      modelScale = rec.data["scale"] as number;
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
        if (currentModel) fitModel(currentModel, modelScale);
      });

      // Follow user mouse movement across the entire desktop (including outside the window)
      let lastRelX = -99999;
      let lastRelY = -99999;

      window.addEventListener("pointermove", (e) => {
        if (!currentModel || dragging) return;
        const hit = currentModel.getBounds().contains(e.clientX, e.clientY);
        wrap.style.cursor = hit ? "pointer" : "grab";
        if (performance.now() < interactUntil) return;
        lastRelX = e.clientX;
        lastRelY = e.clientY;
        focusModel(currentModel, e.clientX, e.clientY);
      });

      const pollDesktopCursor = async (): Promise<void> => {
        if (document.hidden || dragging || !currentModel || performance.now() < interactUntil) return;
        try {
          const pos = await invoke<{ rel_x: number; rel_y: number } | null>(
            "floaty_window_cursor_pos",
            { label: appWin.label },
          );
          if (!pos || !currentModel || dragging || performance.now() < interactUntil) return;
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
        if (!currentModel || performance.now() < interactUntil) return;
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

  // Zoom / scale character with mouse wheel and persist to widget record
  wrap.addEventListener(
    "wheel",
    (e) => {
      if (!currentModel) return;
      e.preventDefault();
      const zoomIn = e.deltaY < 0;
      const factor = zoomIn ? 1.08 : 0.92;
      modelScale = Math.max(0.3, Math.min(3.0, modelScale * factor));
      fitModel(currentModel, modelScale);
      saveScale(modelScale);
    },
    { passive: false },
  );

  // Differentiate drag vs click:
  // - Movement > 5px drags the pinned window
  // - Click / tap triggers model motions (head, body, special) and expressions
  // - Rapid double click triggers special force-priority motion
  let lastTapTime = 0;
  let lastTapPos = { x: 0, y: 0 };

  wrap.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || dragging) return;
    const t = e.target as HTMLElement;
    if (t.closest("button")) return;
    e.stopPropagation();

    const startScreenX = e.screenX;
    const startScreenY = e.screenY;
    const startTime = performance.now();

    let hasDragged = false;
    const posPromise = logicalPos().catch(() => ({ x: 0, y: 0 }));

    const onMove = async (ev: PointerEvent) => {
      const dist = Math.hypot(ev.screenX - startScreenX, ev.screenY - startScreenY);
      if (!hasDragged && dist > 5) {
        hasDragged = true;
        dragging = true;
        try {
          wrap.setPointerCapture(e.pointerId);
        } catch {
          /* ignore */
        }
      }
      if (hasDragged) {
        const p = await posPromise;
        void setLogicalPos(
          Math.round(p.x + (ev.screenX - startScreenX)),
          Math.round(p.y + (ev.screenY - startScreenY)),
        ).catch(() => undefined);
      }
    };

    const onUp = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);

      try {
        if (wrap.hasPointerCapture(e.pointerId)) {
          wrap.releasePointerCapture(e.pointerId);
        }
      } catch {
        /* ignore */
      }

      if (hasDragged) {
        window.setTimeout(() => {
          dragging = false;
        }, 60);
      } else {
        dragging = false;
        const elapsed = performance.now() - startTime;
        if (elapsed < 600 && currentModel) {
          interactUntil = performance.now() + 1500;
          const now = performance.now();
          const isDouble =
            now - lastTapTime < 380 &&
            Math.hypot(ev.clientX - lastTapPos.x, ev.clientY - lastTapPos.y) < 30;
          lastTapTime = now;
          lastTapPos = { x: ev.clientX, y: ev.clientY };

          if (isDouble) {
            handleModelInteract(currentModel, ev.clientX, ev.clientY, "special", MotionPriority.FORCE);
          } else {
            handleModelInteract(currentModel, ev.clientX, ev.clientY, undefined, MotionPriority.NORMAL);
          }
        }
      }
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  });

  wrap.addEventListener("contextmenu", (e) => e.preventDefault());
}
