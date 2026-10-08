import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { appWin, isOverlayMode } from "./windowState";

// Re-export modular subsystems so all existing imports continue seamlessly
export * from "./windowState";
export * from "./slots";
export * from "./geometry";
export * from "./theme";
export * from "./records";
export * from "./pinMenu";

/**
 * Tell the backend this page is alive: after a sleep/standby the app can end up
 * with a wedged renderer, and the only way to tell is that the page stops
 * reporting in (see `recover_windows` in the backend).
 */
export function beatNow(label: string): void {
  void invoke("floaty_heartbeat", { label, visibility: document.visibilityState }).catch(
    () => undefined,
  );
}

export function startHeartbeat(label: string): void {
  const beat = () =>
    void invoke("floaty_heartbeat", { label, visibility: document.visibilityState }).catch(
      () => undefined,
    );
  beat();
  window.setInterval(beat, 5000);
}

/** Pixel size of a stored icon, read straight out of the PNG header. */
function iconSize(url: string): { w: number; h: number } | undefined {
  const b64 = url.startsWith("data:") ? url.slice(url.indexOf(",") + 1) : url;
  try {
    const head = atob(b64.slice(0, 40));
    if (head.length < 24 || head.charCodeAt(0) !== 0x89) return undefined;
    const w =
      ((head.charCodeAt(16) << 24) |
        (head.charCodeAt(17) << 16) |
        (head.charCodeAt(18) << 8) |
        head.charCodeAt(19)) >>>
      0;
    const h =
      ((head.charCodeAt(20) << 24) |
        (head.charCodeAt(21) << 16) |
        (head.charCodeAt(22) << 8) |
        head.charCodeAt(23)) >>>
      0;
    return { w, h };
  } catch {
    return undefined;
  }
}

/**
 * Whether an icon still needs resolving.
 *
 * A stored icon is a file the backend owns, pointed at with an
 * `http://asset.localhost/…` url, so the only honest answer for one of those is
 * the backend's — `floaty_icon` looks for the file itself and re-resolves when
 * it is gone. A legacy data url can still be judged right here, by its PNG
 * header, which is what this used to do for every icon.
 */
export function iconIsMissing(url: string | undefined): boolean {
  if (!url || url === "none") return true;
  if (!url.startsWith("data:")) return false;
  return iconSize(url) === undefined;
}

interface PluginStateMsg {
  id: string;
  enabled: boolean;
}

/** Close this window itself when its plugin is disabled (no main-thread
 * round-trip needed, so it works even when backend window ops stall). */
export function watchPluginEnabled(kind: string): void {
  listen<PluginStateMsg[]>("floaty-plugins-changed", (e) => {
    const p = e.payload.find((p) => p.id === kind);
    if (p && !p.enabled && !isOverlayMode()) {
      void appWin.close().catch(() => undefined);
    }
  }).catch(() => undefined);
}

let live2dCorePromise: Promise<void> | undefined;
/** Loads both Cubism 2 and Cubism 4 core runtimes. Must resolve BEFORE
 * pixi-live2d-display is imported — it throws at module evaluation if
 * either window.Live2D or window.Live2DCubismCore is absent. */
export function ensureLive2DCore(): Promise<void> {
  const win = window as unknown as { Live2D?: unknown; Live2DCubismCore?: unknown };
  if (win.Live2D && win.Live2DCubismCore) return Promise.resolve();
  if (!live2dCorePromise) {
    const loadScript = (src: string) =>
      new Promise<void>((resolve, reject) => {
        const s = document.createElement("script");
        s.src = src;
        s.onload = () => resolve();
        s.onerror = () => reject(new Error(`Failed to load ${src}`));
        document.head.append(s);
      });
    live2dCorePromise = Promise.all([
      win.Live2D ? Promise.resolve() : loadScript("/live2d.min.js"),
      win.Live2DCubismCore ? Promise.resolve() : loadScript("/live2dcubismcore.min.js"),
    ])
      .then(() => undefined)
      .catch((err) => {
        live2dCorePromise = undefined;
        throw err;
      });
  }
  return live2dCorePromise;
}
