import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import * as PIXI from "pixi.js";
import "@pixi/unsafe-eval";
import { Live2DModel, MotionPriority } from "pixi-live2d-display";
import { BASE_H, BASE_W, boxFor, fitInside, maxScaleFor, scaleForBox } from "./live2dBounds";

// Expose PIXI and register ticker so pixi-live2d-display can resolve classes
Live2DModel.registerTicker(PIXI.Ticker);
(window as unknown as { PIXI?: unknown }).PIXI = PIXI;
import {
  addPinMenu,
  appWin,
  currentSettings,
  debounce,
  ensureLive2DCore,
  isOverlayMode,
  loadRecord,
  logicalPos,
  monitorArea,
  notifyDragging,
  overlaySlots,
  refreshSettings,
  removeSelf,
  saveRecord,
  setLogicalPos,
  setWidgetSize,
  trackPosition,
  watchPluginEnabled,
  type MonitorArea,
  type WidgetRecord,
} from "./lib";

type L2DModel = Awaited<ReturnType<typeof Live2DModel.from>>;

/**
 * Tick rates for the model canvas.
 *
 * The Live2D canvas lives inside the full-desktop overlay window, so every
 * canvas update makes Chromium re-present that whole 2880x1920 alpha-blended
 * surface and DWM blend it again — the per-frame cost is dominated by that
 * present, not by the model's own draw calls. The idle loop (breathing) is slow
 * enough that 10fps still reads as smooth; motions and pointer tracking keep
 * the 30fps budget.
 */
const IDLE_FPS = 10;   // breathing loop only
const MOTION_FPS = 20; // a self-triggered gesture is playing
const ACTIVE_FPS = 30; // pointer is on/near the model, or it is being dragged

// Zoom works by resizing the widget box (see widgets/live2dBounds.ts) and
// fitting the model to it, so the model can never grow past the boundary:
// scaling the sprite past its canvas just cropped the character at the edge.

interface AvailableMotion {
  group: string;
  index: number;
  descriptor: string;
  isIdle: boolean;
}

/** Resolves model file path through the asset protocol.
 * Preserving forward slashes ensures pixi-live2d-display correctly resolves relative paths
 * (textures, motions, expressions, moc) to the model's folder rather than the host root.
 */
function resolveModelUrl(model: string): string {
  if (!model) return "";
  if (model.startsWith("http://") || model.startsWith("https://") || model.startsWith("asset://")) return model;
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
  let boxW = BASE_W;
  let boxH = BASE_H;
  let mon: MonitorArea = { x: 0, y: 0, w: window.innerWidth || 1440, h: window.innerHeight || 960 };
  let interactUntil = 0;
  let activeUntil = performance.now() + 3500;
  let motionUntil = 0;

  const wakeFor = (ms: number) => {
    activeUntil = Math.max(activeUntil, performance.now() + ms);
    if (pixiApp && !pixiApp.ticker.started) {
      pixiApp.start();
    }
  };

  const boostActivity = (durationMs = 2000) => {
    wakeFor(durationMs);
    if (pixiApp) pixiApp.ticker.maxFPS = ACTIVE_FPS;
  };

  /** The record the widget is mounted with, kept in sync with the persisted one
   *  so a later save from trackPosition cannot drop the box we just wrote. */
  let liveRec: WidgetRecord | undefined;

  /** Persist the box (and any position nudge) so the backend agrees with the screen. */
  const saveGeometry = debounce((patch: Record<string, number>) => {
    void (async () => {
      const r = liveRec ?? (await loadRecord(id));
      if (!r) return;
      for (const [k, v] of Object.entries(patch)) {
        if (k === "x" || k === "y") r[k] = Math.round(v);
        else r.data[k] = v;
      }
      liveRec = r;
      await saveRecord(r);
    })();
  }, 400);

  /** Fit the model to the widget box. The box is what the zoom changes. */
  const fitModel = (model: L2DModel): void => {
    model.scale.set(1);
    const mw = model.width || (model as unknown as { internalModel?: { width?: number } }).internalModel?.width || 300;
    const mh = model.height || (model as unknown as { internalModel?: { height?: number } }).internalModel?.height || 400;
    const targetW = wrap.clientWidth > 0 ? wrap.clientWidth : (wrap.offsetWidth > 0 ? wrap.offsetWidth : boxW);
    const targetH = wrap.clientHeight > 0 ? wrap.clientHeight : (wrap.offsetHeight > 0 ? wrap.offsetHeight : boxH);
    const s = Math.min(targetW / mw, targetH / mh);
    const baseScale = Number.isFinite(s) && s > 0 ? s * 0.98 : 1;
    model.scale.set(baseScale);
    model.anchor.set(0.5, 0.5);
    model.position.set(targetW / 2, targetH / 2);
  };

  /**
   * Zoom to `requested` by resizing the widget box, then keep that box fully
   * inside the desktop. The model is only ever as big as the box, so this is
   * also what stops it expanding over the boundary: past the desktop edge the
   * zoom simply stops growing (and the widget is nudged back into view).
   */
  const applyZoom = async (requested: number): Promise<void> => {
    try {
      mon = await monitorArea();
    } catch {
      /* keep the last known area */
    }
    const { scale, w, h } = boxFor(requested, mon);
    boxW = w;
    boxH = h;
    modelScale = scale;
    const factor = await appWin.scaleFactor().catch(() => 1);
    setWidgetSize(id, w, h, factor);
    const moved: Record<string, number> = {};
    try {
      const p = fitInside(await logicalPos(id), { w, h }, mon);
      const nx = p.x;
      const ny = p.y;
      if (nx !== p.x || ny !== p.y) {
        void setLogicalPos(nx, ny, id).catch(() => undefined);
        moved["x"] = nx;
        moved["y"] = ny;
      }
    } catch {
      /* keep */
    }
    // the wrap needs a layout pass before it reports its new size
    requestAnimationFrame(() => {
      pixiApp?.resize();
      if (currentModel) fitModel(currentModel);
    });
    saveGeometry({ scale, w, h, ...moved });
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
    boostActivity(5000);
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
    boostActivity(2500);
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
      const model = await Live2DModel.from(url, { autoInteract: false, autoUpdate: false });
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
    liveRec = rec;
    // zoom + box from the record: scale is the box multiplier, w/h the box itself
    const storedW = typeof rec.data["w"] === "number" ? (rec.data["w"] as number) : 0;
    const storedH = typeof rec.data["h"] === "number" ? (rec.data["h"] as number) : 0;
    if (storedW > 0 && storedH > 0) {
      modelScale = scaleForBox(storedW, storedH);
    } else if (typeof rec.data["scale"] === "number" && (rec.data["scale"] as number) > 0) {
      modelScale = rec.data["scale"] as number;
    }
    boxW = storedW > 0 ? storedW : Math.round(BASE_W * modelScale);
    boxH = storedH > 0 ? storedH : Math.round(BASE_H * modelScale);
    // clamp the box (and the position) to the desktop before the model loads
    void applyZoom(modelScale);
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
      PIXI.settings.SCALE_MODE = PIXI.SCALE_MODES.LINEAR;
      PIXI.settings.MIPMAP_TEXTURES = PIXI.MIPMAP_MODES.POW2;
      const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
      pixiApp = new PIXI.Application({
        view: canvas,
        backgroundAlpha: 0,
        antialias: false,
        resolution: dpr,
        autoDensity: true,
        resizeTo: wrap,
        powerPreference: "default",
        sharedTicker: false,
        clearBeforeRender: true,
      });
      pixiApp.ticker.maxFPS = IDLE_FPS;

      let tickCount = 0;
      pixiApp.ticker.add(() => {
        if (tickCount++ === 0) {
          invoke("floaty_log", { msg: `[live2d] TICKER FIRST TICK, started=${pixiApp?.ticker.started}` }).catch(() => undefined);
        }
        if (currentModel) {
          currentModel.update(pixiApp!.ticker.deltaMS);
        }
        const now = performance.now();
        const targetFps =
          now < activeUntil ? ACTIVE_FPS : now < motionUntil ? MOTION_FPS : IDLE_FPS;
        if (pixiApp!.ticker.maxFPS !== targetFps) {
          pixiApp!.ticker.maxFPS = targetFps;
        }
      });
      let m = typeof rec.data["model"] === "string" ? (rec.data["model"] as string) : "";
      if (!m) {
        try {
          await refreshSettings();
          let root = currentSettings().live2d_root || "";
          if (!root) {
            const s = await invoke<{ live2d_root?: string }>("floaty_get_settings");
            root = s.live2d_root || "";
          }
          if (root) {
            const list = await invoke<Array<{ name: string; path: string }>>("floaty_scan_models", { root });
            if (list && list.length > 0) {
              m = list[0].path;
              rec.data["model"] = m;
              void saveRecord(rec);
            }
          }
        } catch {
          /* ignore */
        }
      }
      const modelUrl = resolveModelUrl(m);
      if (modelUrl) {
        await showModel(modelUrl);
      } else {
        failedBox();
      }
      window.addEventListener("resize", () => {
        boostActivity(1500);
        void applyZoom(modelScale);
      });

      // Follow user mouse movement smoothly across the desktop
      let lastRelX = -99999;
      let lastRelY = -99999;
      let lastFarGlance = 0;

      wrap.addEventListener("pointermove", (e) => {
        if (!currentModel || dragging) return;
        wakeFor(2000);
        const r = wrap.getBoundingClientRect();
        const localX = isOverlayMode() ? e.clientX - r.left : e.clientX;
        const localY = isOverlayMode() ? e.clientY - r.top : e.clientY;
        const hit = currentModel.getBounds().contains(localX, localY);
        wrap.style.cursor = hit ? "pointer" : "grab";
        if (performance.now() < interactUntil) return;
        lastRelX = localX;
        lastRelY = localY;
        focusModel(currentModel, localX, localY);
      });

      const pollDesktopCursor = async (): Promise<void> => {
        if (document.hidden || dragging || !currentModel || performance.now() < interactUntil) return;
        try {
          const pos = await invoke<{ rel_x: number; rel_y: number } | null>(
            "floaty_window_cursor_pos",
            { label: appWin.label },
          );
          if (!pos || !currentModel || dragging || performance.now() < interactUntil) return;
          const slot = isOverlayMode() ? overlaySlots.get(id) : undefined;
          const slotX = slot ? slot.x : wrap.getBoundingClientRect().left;
          const slotY = slot ? slot.y : wrap.getBoundingClientRect().top;
          const relX = isOverlayMode() ? pos.rel_x - slotX : pos.rel_x;
          const relY = isOverlayMode() ? pos.rel_y - slotY : pos.rel_y;
          const dx = relX - lastRelX;
          const dy = relY - lastRelY;
          const dist = Math.hypot(dx, dy);

          // Check proximity to Live2D window (300x400)
          const targetW = wrap.clientWidth || 300;
          const targetH = wrap.clientHeight || 400;
          const nearX = relX >= -160 && relX <= targetW + 160;
          const nearY = relY >= -160 && relY <= targetH + 160;
          const isNear = nearX && nearY;

          if (isNear) {
            // Near cursor: fluid 30fps tracking
            if (dist < 3) return;
            lastRelX = relX;
            lastRelY = relY;
            focusModel(currentModel, relX, relY);
            wakeFor(1500);
          } else {
            // Far cursor across desktop: glance toward cursor when it moves significantly
            const now = performance.now();
            if (dist < 60 || now - lastFarGlance < 2500) return;
            lastFarGlance = now;
            lastRelX = relX;
            lastRelY = relY;
            focusModel(currentModel, relX, relY);
            wakeFor(600);
          }
        } catch {
          /* ignore */
        }
      };
      trackingTimer = window.setInterval(() => {
        void pollDesktopCursor();
      }, 120);

      document.addEventListener("visibilitychange", () => {
        if (!pixiApp) return;
        if (document.hidden) {
          pixiApp.stop();
        } else {
          wakeFor(2000);
        }
      });

      // Periodic natural motion change
      window.setInterval(() => {
        if (!currentModel || performance.now() < interactUntil) return;
        // a gesture needs more than the breathing rate, but far less than pointer
        // tracking: every extra frame here is a full-desktop composite
        motionUntil = performance.now() + 3000;
        wakeFor(500);
        try {
          void currentModel.motion("idle", undefined, MotionPriority.IDLE);
        } catch {
          /* model idles on its own */
        }
      }, 15000);
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
      boostActivity(1500);
      const zoomIn = e.deltaY < 0;
      const factor = zoomIn ? 1.08 : 0.92;
      // applyZoom clamps to the desktop, so the model stops growing at the edge
      void applyZoom(modelScale * factor);
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

    const isOverlay = isOverlayMode();
    const startScreenX = isOverlay ? e.clientX : e.screenX;
    const startScreenY = isOverlay ? e.clientY : e.screenY;
    const startTime = performance.now();

    let hasDragged = false;
    const posPromise = logicalPos(id).catch(() => ({ x: 0, y: 0 }));

    const onMove = async (ev: PointerEvent) => {
      const curSX = isOverlay ? ev.clientX : ev.screenX;
      const curSY = isOverlay ? ev.clientY : ev.screenY;
      const dist = Math.hypot(curSX - startScreenX, curSY - startScreenY);
      if (!hasDragged && dist > 5) {
        hasDragged = true;
        dragging = true;
        notifyDragging(true);
        try {
          wrap.setPointerCapture(e.pointerId);
        } catch {
          /* ignore */
        }
      }
      if (hasDragged) {
        boostActivity(1000);
        const p = await posPromise;
        // clamp to the desktop: dragging the model off the boundary is exactly
        // what leaves half of it outside the screen
        const dest = fitInside(
          { x: p.x + (curSX - startScreenX), y: p.y + (curSY - startScreenY) },
          { w: boxW, h: boxH },
          mon,
        );
        void setLogicalPos(Math.round(dest.x), Math.round(dest.y), id).catch(() => undefined);
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
          notifyDragging(false);
        }, 60);
      } else {
        dragging = false;
        notifyDragging(false);
        const elapsed = performance.now() - startTime;
        if (elapsed < 600 && currentModel) {
          const r = wrap.getBoundingClientRect();
          const localX = isOverlay ? ev.clientX - r.left : ev.clientX;
          const localY = isOverlay ? ev.clientY - r.top : ev.clientY;
          interactUntil = performance.now() + 1500;
          const now = performance.now();
          const isDouble =
            now - lastTapTime < 380 &&
            Math.hypot(localX - lastTapPos.x, localY - lastTapPos.y) < 30;
          lastTapTime = now;
          lastTapPos = { x: localX, y: localY };

          if (isDouble) {
            handleModelInteract(currentModel, localX, localY, "special", MotionPriority.FORCE);
          } else {
            handleModelInteract(currentModel, localX, localY, undefined, MotionPriority.NORMAL);
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

if (import.meta.hot) {
  import.meta.hot.accept(() => {
    window.location.reload();
  });
}

