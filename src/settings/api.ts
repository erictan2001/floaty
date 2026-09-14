/**
 * The two things every settings pane needs from the backend: a call that
 * reports its own failures, and the banner they are reported in.
 *
 * Failures are reported, never thrown: a pane that dies on the first refused
 * call leaves the window half-drawn, which reads as "settings is broken".
 */

import { invoke } from "@tauri-apps/api/core";

let bar: HTMLElement | undefined;
let hideTimer: number | undefined;

/** Put the error banner at the top of the window. Hidden until something fails. */
export function mountErrorBar(host: HTMLElement): void {
  bar = document.createElement("div");
  bar.className = "error-bar";
  host.prepend(bar);
}

export function showError(msg: string): void {
  // Also the fallback path: if the shell never got far enough to mount the
  // banner, one is created so the message is not swallowed.
  if (!bar) {
    bar = document.createElement("div");
    bar.className = "error-bar";
    document.body.prepend(bar);
  }
  bar.textContent = msg;
  bar.style.display = "block";
  window.clearTimeout(hideTimer);
  hideTimer = window.setTimeout(() => {
    if (bar) bar.style.display = "none";
  }, 6000);
}

/** Append to floaty.log — the only place this window can be debugged from. */
export function log(msg: string): void {
  void invoke("floaty_log", { msg }).catch(() => undefined);
}

/**
 * Run one backend call, logging its start and end.
 *
 * The markers earn their keep when a call never settles: a start line with no
 * matching end line in floaty.log names the stuck command, which is otherwise
 * invisible from the outside.
 */
export async function safe<T>(label: string, fn: () => Promise<T>): Promise<T | undefined> {
  log(`[settings] invoke-start: ${label}`);
  try {
    const result = await fn();
    log(`[settings] invoke-end: ${label}`);
    return result;
  } catch (err) {
    log(`[settings] invoke-error: ${label}`);
    showError(`${label}: ${err instanceof Error ? err.message : String(err)}`);
    return undefined;
  }
}
