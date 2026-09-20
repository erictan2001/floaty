/**
 * What floaty can say about itself: the presence state, the sizes on disk, the
 * monitors, the windows it owns, the heartbeat table, and the end of the log.
 *
 * This pane exists for the moment something is wrong and the window is the only
 * thing still answering. Every number here is one a bug report has needed — which
 * monitors exist and at what scale, where each window actually is, whether a page
 * still believes it is hidden, how big the things on disk have grown — and the log
 * tail is selectable so the one line that matters can be copied out of it.
 */

import { invoke } from "@tauri-apps/api/core";
import { safe } from "../api";
import { action, actionRow, card, el, group, note } from "../dom";
import type { Pane } from "../pane";

/** The store and whatever copies sit beside it. */
interface FileInfo {
  path: string;
  bytes: number;
  backups: number;
}

interface DirInfo {
  path: string;
  files: number;
  bytes: number;
}

interface LogInfo {
  path: string;
  bytes: number;
  /** The tail of the log, oldest line first. */
  lines: string[];
  /** How many `floaty.log.N` files are sitting beside it. */
  rotated: number;
}

interface MonitorInfo {
  /** The device name, which is what stays put across replugs (`\\.\DISPLAY1`). */
  key: string;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
  primary: boolean;
}

interface WindowInfo {
  label: string;
  /** What the app did with it last: hidden windows are meant to be hidden. */
  visible: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
  /** "desktop" (the desktop itself, behind the icons) or "window". */
  layer: string;
}

interface BeatInfo {
  label: string;
  /** What the page reports about itself: "visible" or "hidden". */
  visibility: string;
  ms_ago: number;
}

interface RepairInfo {
  label: string;
  attempts: number;
}

/** The report `floaty_diagnostics` returns, as the backend serialises it. */
interface DiagnosticsReport {
  version: string;
  mode: string;
  os: string;
  /** Uptime in ms, so this window needs no timer of its own to say how long. */
  started_ms_ago: number;
  log: LogInfo;
  store: FileInfo;
  icons: DirInfo;
  records: number;
  installed_plugins: string[];
  rejected_plugins: string[];
  /** "on" | "off" | "unknown" — whether the screen is on, as floaty tracks it. */
  display: string;
  heartbeats: BeatInfo[];
  repairs: RepairInfo[];
  monitors: MonitorInfo[];
  windows: WindowInfo[];
}

/** The state `floaty_presence` returns: why the desktop is hidden or quiet right now. */
interface PresenceReport {
  /** The desktop is off screen, because a fullscreen app is in front. */
  hidden: boolean;
  /** The floaties stay put, but stop animating and pause the audio capture. */
  quiet: boolean;
  /** Why: "fullscreen", "idle", or null when the desktop is behaving normally. */
  reason: string | null;
  idle_ms: number;
  /** The window in front, in physical px — "1920x1080 at 0,0". */
  foreground: string | null;
  /** That window is the shell (desktop, taskbar, start menu) — never a reason to hide. */
  foreground_shell: boolean;
  /** ...or one of floaty's own windows, which is also never a reason to hide. */
  foreground_ours: boolean;
  hide_in_fullscreen: boolean;
  quiet_when_idle: boolean;
  /** How long "a while" is, in minutes; 0 means never. */
  idle_minutes: number;
}

let host: HTMLElement | undefined;
let status: HTMLElement | undefined;

/** Sizes in a unit a person reads: 1887436 bytes is not a number anyone can hold. */
function human(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/** "1h 12m": the two largest units, because a report is read rather than computed with. */
function uptime(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds % 60}s`;
  return `${seconds}s`;
}

/** How long ago a beat arrived. */
function ago(ms: number): string {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  return `${Math.round(minutes / 60)}h ago`;
}

/** 1.25 → "1.25x", 1 → "1x": a scale factor is a multiplier, so say so. */
function scaleText(scale: number): string {
  return `${scale.toFixed(2).replace(/\.?0+$/, "")}x`;
}

/** "1 backup" / "3 backups", so no line reads as a database dump. */
function count(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}

/** One fact: its name on the left, its value against the right edge. */
function row(label: string, value: string, valueCls = "diag-val"): HTMLElement {
  const line = el("div", "set-row diag-row");
  line.append(el("span", "set-label diag-key", label), el("span", valueCls, value));
  return line;
}

/** A path on a line of its own: too long for a value cell, and the ellipsis keeps it there. */
function pathLine(path: string): HTMLElement {
  const line = el("div", "set-row");
  const span = el("span", "diag-path", path);
  // The ellipsis hides the tail, and the tail is what names the file.
  span.title = path;
  line.append(span);
  return line;
}

/** A file or folder with its size: what it is, how big, and where. */
function diskCard(label: string, path: string, detail: string): HTMLElement {
  const item = card(true);
  item.append(row(label, detail), pathLine(path));
  return item;
}

/**
 * Open the folder the log lives in. A refusal is the pane's news, not an
 * exception: the rest of the report is why the user is here, and losing it to an
 * Explorer that would not start would be the worst possible trade.
 */
async function openLogFolder(): Promise<void> {
  try {
    await invoke("floaty_open_log_folder");
    if (status?.isConnected) status.textContent = "opened the folder holding floaty.log";
  } catch (err) {
    if (status?.isConnected) status.textContent = `could not open the log folder: ${String(err)}`;
  }
}

/**
 * Bring back anything sitting at coordinates no screen covers.
 *
 * The backend does this by itself when Windows says the arrangement changed and once
 * at startup; this is the by-hand version, for a floatie that went missing from a
 * display that is no longer plugged in and a user who would rather not restart.
 */
async function bringBackOffscreen(): Promise<void> {
  try {
    const moved = await invoke<number>("floaty_rehome_floaties");
    if (status?.isConnected) {
      status.textContent =
        moved === 0
          ? "every floatie is already on a screen"
          : `brought ${moved} floatie(s) back from a screen that is gone`;
    }
    if (moved > 0) await draw();
  } catch (err) {
    if (status?.isConnected) status.textContent = `could not bring them back: ${String(err)}`;
  }
}

/**
 * One fact with a short marker ahead of its value, for the case where the value
 * needs a reason spelled out beside it: the foreground window's marker says why it
 * was, or was not, something that hides the desktop.
 */
function markedRow(label: string, value: string, mark: string, markCls: string): HTMLElement {
  const line = el("div", "set-row diag-row");
  line.append(
    el("span", "set-label diag-key", label),
    el("span", markCls, mark),
    el("span", "diag-val", value),
  );
  return line;
}

/** The window in front, and whether the shell or one of floaty's own windows owns it. */
function foregroundRow(presence: PresenceReport): HTMLElement {
  if (!presence.foreground) return row("foreground", "no window in front");
  // Shell and ours are the two that are never a reason to hide, so they get the
  // marker; an ordinary app needs none, since the state row already says what
  // happened to the desktop because of it.
  if (presence.foreground_shell) {
    return markedRow("foreground", presence.foreground, "shell", "diag-layer");
  }
  if (presence.foreground_ours) {
    return markedRow("foreground", presence.foreground, "ours", "diag-layer diag-desktop");
  }
  return row("foreground", presence.foreground);
}

/**
 * The live presence state and the rules behind it: why the desktop is hidden or
 * quiet, and what was in front when it decided.
 *
 * Read-only, and drawn from the same refresh as the rest of the report — a snapshot
 * taken when the pane draws, not a timer of its own.
 */
function presenceSection(presence: PresenceReport | null | undefined): HTMLElement {
  const section = group(
    "Getting out of the way",
    "hiding and going quiet are separate answers. Hidden means a fullscreen app is in front and the overlays are off screen; quiet means nobody has touched the machine for a while, so the floaties stay put but stop animating and the audio capture pauses. A maximized window is not fullscreen — it leaves the taskbar visible — so a maximized window does not hide the desktop.",
  );
  if (!presence) {
    section.body.append(
      note("the presence state could not be read: floaty_presence did not answer."),
    );
    return section.root;
  }
  const state = presence.hidden ? "hidden" : presence.quiet ? "quiet" : "normal";
  const facts = card(true);
  facts.append(
    row("state", state),
    row("reason", presence.reason ?? "none"),
    // "2m 14s" reads; a millisecond count does not.
    row("idle", uptime(presence.idle_ms)),
    foregroundRow(presence),
    row("hide for a fullscreen app", presence.hide_in_fullscreen ? "on" : "off"),
    row("go quiet when idle", presence.quiet_when_idle ? "on" : "off"),
    row("idle after", presence.idle_minutes === 0 ? "never" : `${presence.idle_minutes} min`),
  );
  section.body.append(facts);
  return section.root;
}

async function draw(): Promise<void> {
  if (!host || !host.isConnected) return;
  host.innerHTML = "";
  status = undefined;

  const tools = actionRow();
  tools.append(
    action("open the log folder", openLogFolder),
    action("bring back off-screen floaties", bringBackOffscreen, { busyLabel: "looking…" }),
    action("refresh", () => draw(), { busyLabel: "reading…" }),
  );
  const line = note("");
  status = line;
  host.append(tools, line);

  // The two reads are independent, so they go together: one round trip's latency for
  // both, and both ride the same refresh path the toolbar's button drives.
  const [report, presence] = await Promise.all([
    safe("read diagnostics", () => invoke<DiagnosticsReport>("floaty_diagnostics")),
    safe("read presence", () => invoke<PresenceReport>("floaty_presence")),
  ]);
  // A tab click or a data change can re-render the pane while this call is in
  // flight, so the answer is only painted onto a host that is still the one asked.
  if (!host || !host.isConnected) return;
  if (!report) {
    const failure = group("This app");
    failure.body.append(note("the report could not be read: floaty_diagnostics did not answer."));
    host.append(failure.root);
    return;
  }

  const presenceEl = presenceSection(presence);

  const facts = card(true);
  facts.append(
    row("version", report.version),
    row("mode", report.mode),
    row("os", report.os),
    row("uptime", uptime(report.started_ms_ago)),
    row("records", `${report.records}`),
    row("display", report.display),
    row("installed plugins", report.installed_plugins.join(", ") || "none"),
  );
  // Rejections are the one fact here with a cause worth spelling out, so the line
  // is only drawn when there is something to explain.
  if (report.rejected_plugins.length > 0) {
    const rejected = el("div", "diag-stack");
    rejected.append(
      row("rejected plugins", report.rejected_plugins.join(", "), "diag-val diag-warn"),
      note(
        "A rejected plugin is a folder under %APPDATA%\\com.floaty.app\\plugins that floaty refused to load: its plugin.json did not parse, its api version is not one this build knows, or its index.js threw while it was imported.",
      ),
    );
    facts.append(rejected);
  }
  const app = group(
    "This app",
    "mode is desktop when floaty draws behind the icons and floating when the overlay is a window of its own; display is whether the screen is on, as floaty last heard from Windows.",
  );
  app.body.append(facts);

  const disk = group("On disk");
  disk.body.append(
    diskCard(
      "floaty-store.json",
      report.store.path,
      `${human(report.store.bytes)}, ${count(report.store.backups, "backup")}`,
    ),
    diskCard(
      "icons",
      report.icons.path,
      `${count(report.icons.files, "file")}, ${human(report.icons.bytes)}`,
    ),
    diskCard(
      "floaty.log",
      report.log.path,
      `${human(report.log.bytes)}, ${count(report.log.rotated, "rotated file")}`,
    ),
    note("floaty.log.1 is the previous file: the log rolls over at 1 MB, so the lines from just before a restart are still there."),
  );

  const only = report.monitors.length === 1 ? report.monitors[0] : undefined;
  const monitors = group(
    "Monitors",
    only
      ? `one monitor — the overlay covers ${only.x},${only.y} ${only.width}x${only.height} at ${scaleText(only.scale)}`
      : "one row per display floaty can see. The desktop covers the union of them, so a floatie can live on any screen and be dragged between them.",
  );
  if (report.monitors.length === 0) {
    monitors.body.append(note("no monitor was reported, so nothing can be placed on the desktop."));
  }
  for (const monitor of report.monitors) {
    const item = card(true);
    const head = el("div", "set-row diag-row");
    head.append(el("span", "set-label diag-key", monitor.name || monitor.key));
    if (monitor.primary) head.append(el("span", "diag-primary", "primary"));
    item.append(
      head,
      el("p", "set-sub", `${monitor.x},${monitor.y} ${monitor.width}x${monitor.height} at ${scaleText(monitor.scale)}`),
    );
    monitors.body.append(item);
  }

  const windows = group(
    "Windows",
    "desktop marks the desktop-layer windows: they are the desktop itself, drawn behind every icon. Anything else is an ordinary window. Positions are physical px, as the app last put them.",
  );
  if (report.windows.length === 0) {
    windows.body.append(note("floaty owns no windows right now."));
  }
  for (const win of report.windows) {
    const item = card(true);
    const head = el("div", "set-row diag-row");
    head.append(
      el("span", "set-label diag-key", win.label),
      el("span", "diag-state", win.visible ? "visible" : "hidden"),
    );
    const place = el("div", "diag-line");
    place.append(
      el("span", win.layer === "desktop" ? "diag-layer diag-desktop" : "diag-layer", win.layer),
      el("span", "diag-val", `${win.x},${win.y} ${win.width}x${win.height}`),
    );
    item.append(head, place);
    windows.body.append(item);
  }

  const repairs = new Map(report.repairs.map((repair) => [repair.label, repair.attempts]));
  const beats = group(
    "Heartbeats",
    "One row per window: the visibility its page reports, and how long ago its last beat arrived. A page that believes it is hidden while the display is on gets forced visible, then rebuilt — repaired counts how often that has been done to it.",
  );
  if (report.heartbeats.length === 0) {
    beats.body.append(note("no window has beaten yet."));
  }
  for (const beat of report.heartbeats) {
    const item = card(true);
    const head = el("div", "set-row diag-row");
    const attempts = repairs.get(beat.label) ?? 0;
    head.append(el("span", "set-label diag-key", beat.label));
    if (attempts > 0) head.append(el("span", "diag-mark", `repaired ${attempts}x`));
    head.append(el("span", "diag-val", `${beat.visibility} — ${ago(beat.ms_ago)}`));
    item.append(head);
    beats.body.append(item);
  }

  const tail = group("Log tail", "the end of floaty.log, oldest line first");
  tail.body.append(
    el("pre", "diag-log", report.log.lines.length > 0 ? report.log.lines.join("\n") : "the log is empty"),
  );

  host.append(presenceEl, app.root, disk.root, monitors.root, windows.root, beats.root, tail.root);
}

export const diagnosticsPane: Pane = {
  id: "diagnostics",
  label: "Diagnostics",
  title: "Diagnostics",
  hint: "What floaty can say about itself: whether the desktop is hidden or quiet, the sizes on disk, the monitors and windows it owns, the heartbeat table, and the end of the log.",
  render(node) {
    host = node;
    void draw();
  },
};
