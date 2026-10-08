import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { rippleIndex } from "./placement";
import { appWin } from "./windowState";
import { overlaySlots } from "./slots";
import { clampNum, monitorAt, onPresenceChange, presenceIsQuiet } from "./geometry";

export interface FloatSettings {
  gravity: number;
  bounce: number;
  single_click: string;
  double_click: string;
  live2d_root: string;
  files_root: string;
  disabled: string[];
  stay_on_desktop: boolean;
  /** launch floaty when the user signs in */
  start_on_boot: boolean;
  /** percentage of the desktop items that bob at all */
  animated_ratio: number;
  animation_mode: string;
  /** how far a resting icon rises, in px — the same travel in every mode */
  float_amplitude: number;
  /** seconds per bob */
  float_period: number;
  /** wave only: how much of the cycle separates one icon from the next, in % */
  float_spread: number;
  /** ask before removing a file, folder or widget */
  confirm_remove: boolean;
  /** visualizer sensitivity multiplier */
  viz_gain: number;
  /** visualizer redraw rate while audio plays */
  viz_fps: number;
  /** system monitor sample period in ms */
  sysmon_interval: number;
  /** the key that opens the launcher palette, e.g. "Ctrl+Alt+Space" */
  palette_shortcut: string;
  /** hide the desktop while a fullscreen app is in front */
  hide_in_fullscreen: boolean;
  /** stop animating when nobody has touched the machine */
  quiet_when_idle: boolean;
  /** how long "a while" is, in minutes; 0 means never */
  idle_minutes: number;
  /**
   * Whether the user has answered the first-run question about the root (ADR 0004).
   * Read, never written from here: the settings window has no row for it, the
   * backend keeps it against a save, and `floaty_confirm_root` is the only thing
   * that sets it.
   */
  root_confirmed: boolean;
}

export const DEFAULT_SETTINGS: FloatSettings = {
  gravity: 2600,
  bounce: 0.45,
  single_click: "nothing",
  double_click: "launch",
  live2d_root: "",
  files_root: "",
  disabled: [],
  stay_on_desktop: true,
  start_on_boot: false,
  animated_ratio: 100,
  animation_mode: "wave",
  float_amplitude: 6,
  float_period: 10,
  float_spread: 10,
  confirm_remove: true,
  viz_gain: 1,
  viz_fps: 30,
  sysmon_interval: 1000,
  palette_shortcut: "Ctrl+Alt+Space",
  hide_in_fullscreen: true,
  quiet_when_idle: true,
  idle_minutes: 10,
  root_confirmed: false,
};

/** A settings number, or the fallback when the field is missing or not one. */
export function settingNum(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

/**
 * Applies the float settings to one desktop item: its mode, its travel and
 * period, and — in wave mode — where it sits in the ripple.
 */
export function applyFloatieAnimation(
  wrap: HTMLElement,
  id: string,
  at?: { x: number; y: number },
): void {
  const s = currentSettings();
  const height = clampNum(settingNum(s.float_amplitude, 6), 0, 24);
  const period = clampNum(settingNum(s.float_period, 10), 2, 60);
  const ratio = clampNum(settingNum(s.animated_ratio, 100), 0, 100);
  const spread = clampNum(settingNum(s.float_spread, 10), 0, 100);
  const mode = s.animation_mode || "wave";

  if (presenceIsQuiet()) {
    wrap.classList.remove("anim-wave", "anim-sync", "anim-gentle", "anim-static");
    wrap.classList.add("anim-static");
    return;
  }

  wrap.style.setProperty("--bob-amp", `${height}px`);
  wrap.style.setProperty("--bob-cycle", `${period}s`);
  const ripple = mode === "wave" && at ? rippleIndex(at, monitorAt(at.x)) : 0;
  wrap.style.setProperty("--bob-delay", `${((-ripple * spread) / 100) * period}s`);

  wrap.classList.remove("anim-wave", "anim-sync", "anim-gentle", "anim-static");

  const logDecision = (decision: string): void => {
    if (wrap.dataset.motionLogged === decision) return;
    wrap.dataset.motionLogged = decision;
    void invoke("floaty_log", {
      msg: `[motion] ${id} -> ${decision} (${describeMotion(s)})`,
    }).catch(() => undefined);
  };

  if (mode === "static" || ratio <= 0 || height <= 0) {
    wrap.classList.add("anim-static");
    logDecision("static");
    return;
  }

  const numDigits = parseInt(id.replace(/\D/g, ""), 10);
  const num = Number.isFinite(numDigits)
    ? numDigits
    : Math.abs(Array.from(id).reduce((acc, c) => (acc * 31 + c.charCodeAt(0)) | 0, 0));

  const isAnimated = (num * 37) % 100 < ratio;
  if (!isAnimated) {
    wrap.classList.add("anim-static");
    logDecision("static");
    return;
  }

  wrap.classList.add(`anim-${mode}`);
  logDecision(ripple > 0 ? `${mode} #${ripple}` : mode);
}

// Re-apply animation when machine presence changes
onPresenceChange(() => {
  for (const slot of overlaySlots.values()) {
    applyFloatieAnimation(slot.element, slot.id, { x: slot.x, y: slot.y });
  }
});

let settingsCache: FloatSettings = { ...DEFAULT_SETTINGS };
let settingsWatched = false;
let settingsReady = false;
const settingsConsumers = new Set<() => void>();

export function currentSettings(): FloatSettings {
  return settingsCache;
}

export function onSettings(cb: () => void): void {
  settingsConsumers.add(cb);
  if (settingsReady) cb();
}

function applySettings(): void {
  for (const cb of settingsConsumers) {
    try {
      cb();
    } catch {
      /* one bad consumer must not stop the others */
    }
  }
}

function describeMotion(s: FloatSettings): string {
  return `ratio=${s.animated_ratio} mode=${s.animation_mode} height=${s.float_amplitude} period=${s.float_period} spread=${s.float_spread}`;
}
let lastMotion = "";

export async function refreshSettings(): Promise<void> {
  try {
    const s = await invoke<FloatSettings>("floaty_get_settings");
    settingsCache = { ...DEFAULT_SETTINGS, ...s };
    settingsReady = true;
  } catch {
    /* keep last */
  }
  applySettings();
}

export function watchSettings(): void {
  if (settingsWatched) return;
  settingsWatched = true;
  void refreshSettings();
  listen<FloatSettings>("floaty-settings-changed", (e) => {
    settingsCache = { ...DEFAULT_SETTINGS, ...e.payload };
    settingsReady = true;
    const motion = describeMotion(settingsCache);
    if (motion !== lastMotion) {
      lastMotion = motion;
      void invoke("floaty_log", { msg: `[motion] ${appWin.label} settings: ${motion}` }).catch(
        () => undefined,
      );
    }
    applySettings();
  }).catch(() => undefined);
}
