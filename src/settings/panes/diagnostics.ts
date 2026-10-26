/**
 * What floaty can say about itself: the presence state, the sizes on disk, the
 * monitors, the windows it owns, the heartbeat table, and the end of the log.
 *
 * The top of the pane is what a person checks first — is the desktop hidden or
 * quiet, and why. Everything a bug report needs but nobody reads by accident is
 * one click away in a folded row, and the folded row's summary carries the one
 * number that says whether opening it is worth it.
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
interface PresenceScreen {
  /** The desktop layer's label — `desktop-overlay`, `desktop-overlay-1`, … */
  label: string;
  /** The monitor's own name, when the label maps to one. */
  screen: string | null;
  hidden: boolean;
  quiet: boolean;
  reason: string | null;
}

interface PresenceReport {
  /** Hidden on *any* screen — `screens` says which. */
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
  /** One entry per desktop layer: the decision is per screen, because the windows are. */
  screens: PresenceScreen[];
  hide_in_fullscreen: boolean;
  quiet_when_idle: boolean;
  /** How long "a while" is, in minutes; 0 means never. */
  idle_minutes: number;
}

let host: HTMLElement | undefined;
let status: HTMLElement | undefined;
/** The last action's news, kept across a redraw so "opened the log folder" is
 *  still there after the refresh that follows it. */
let message = "";

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

/**
 * One compact labelled value, two or more to a row. `value` is also the tooltip,
 * because a grid cell ellipsizes what does not fit and the full text still matters.
 */
function fact(label: string, value: string, cls = ""): HTMLElement {
  const box = el("div", "diag-fact");
  const val = el("b", cls, value);
  val.title = value;
  box.append(el("span", "", label), val);
  return box;
}

/** A wrapping grid of facts. */
function facts(items: HTMLElement[]): HTMLElement {
  const grid = el("div", "diag-grid");
  grid.append(...items);
  return grid;
}

/**
 * A section that opens on a click: the title and a one-line summary sit in the
 * closed row, so hiding the detail never hides the number that says it is boring.
 */
function fold(title: string, summary: string): { root: HTMLDetailsElement; body: HTMLElement } {
  const root = el("details", "set-group set-fold");
  const head = el("summary", "set-fold-head");
  head.append(el("span", "set-group-title", title), el("span", "diag-summary", summary));
  const body = el("div", "set-group-body");
  root.append(head, body);
  return { root, body };
}

function say(text: string): void {
  message = text;
  if (status?.isConnected) status.textContent = text;
}

/**
 * Open the folder the log lives in. A refusal is the pane's news, not an
 * exception: the rest of the report is why the user is here, and losing it to an
 * Explorer that would not start would be the worst possible trade.
 */
async function openLogFolder(): Promise<void> {
  try {
    await invoke("floaty_open_log_folder");
    say("opened the folder holding floaty.log");
  } catch (err) {
    say(`could not open the log folder: ${String(err)}`);
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
    say(
      moved === 0
        ? "every floatie is already on a screen"
        : `brought ${moved} floatie(s) back from a screen that is gone`,
    );
    if (moved > 0) await draw();
  } catch (err) {
    say(`could not bring them back: ${String(err)}`);
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
  const section = group("Now");
  if (!presence) {
    section.body.append(
      note("the presence state could not be read: floaty_presence did not answer."),
    );
    return section.root;
  }
  const screens = presence.screens ?? [];
  const hiddenCount = screens.filter((s) => s.hidden).length;
  const state = presence.hidden ? "hidden" : presence.quiet ? "quiet" : "normal";
  const headline =
    state === "hidden"
      ? hiddenCount > 1
        ? `Hidden on ${hiddenCount} of ${screens.length} screens`
        : "Hidden — a fullscreen app is in front"
      : state === "quiet"
        ? `Quiet — no input for ${uptime(presence.idle_ms)}`
        : "On screen — nothing is in the way";
  const item = card(true);
  const head = el("div", "diag-headline");
  head.append(el("span", `diag-dot ${state}`), el("span", "set-label", headline));

  const rules = [
    fact("fullscreen", presence.hide_in_fullscreen ? "hide" : "stays"),
    fact(
      "idle quiet",
      presence.quiet_when_idle
        ? presence.idle_minutes === 0
          ? "never"
          : `after ${presence.idle_minutes} min`
        : "off",
    ),
  ];

  item.append(
    head,
    facts([fact("idle", uptime(presence.idle_ms)), ...rules]),
    foregroundRow(presence),
  );
  // One line per desktop layer. A fullscreen app on one screen takes that screen's desktop
  // away and leaves every other screen alone, so this is the field that shows the rule.
  if (screens.length > 1) {
    const perScreen = el("div", "diag-grid");
    for (const screen of screens) {
      const what = screen.hidden ? "hidden" : screen.quiet ? "quiet" : "on screen";
      const why = screen.reason ? ` — ${screen.reason}` : "";
      perScreen.append(fact(screen.screen || screen.label, `${what}${why}`));
    }
    item.append(perScreen);
  }
  section.body.append(item);
  return section.root;
}

async function draw(): Promise<void> {
  if (!host || !host.isConnected) return;
  host.innerHTML = "";
  status = undefined;

  // Refresh is the one action here a person presses on purpose; the maintenance
  // pair are quiet so the toolbar is not three competing primary buttons.
  const tools = actionRow();
  tools.classList.add("set-toolbar");
  tools.append(
    action("refresh", () => draw(), { busyLabel: "reading…" }),
    action("bring back off-screen floaties", bringBackOffscreen, {
      cls: "pill ghost",
      busyLabel: "looking…",
    }),
    action("open the log folder", openLogFolder, { cls: "pill ghost" }),
  );
  status = el("p", "set-sub set-status", message);
  host.append(tools, status);

  // The two reads are independent, so they go together: one round trip's latency for
  // both, and both ride the same refresh path the toolbar's button drives.
  const [report, presence] = await Promise.all([
    safe("read diagnostics", () => invoke<DiagnosticsReport>("floaty_diagnostics")),
    safe("read presence", () => invoke<PresenceReport>("floaty_presence")),
  ]);
  // A section switch or a data change can re-render the pane while this call is in
  // flight, so the answer is only painted onto a host that is still the one asked.
  if (!host || !host.isConnected) return;
  if (!report) {
    const failure = group("This app");
    failure.body.append(note("the report could not be read: floaty_diagnostics did not answer."));
    host.append(failure.root);
    return;
  }

  const app = fold(
    "This app",
    `${report.version} · ${report.mode} · up ${uptime(report.started_ms_ago)}`,
  );
  app.body.append(
    facts([
      fact("version", report.version),
      fact("mode", report.mode),
      fact("os", report.os),
      fact("uptime", uptime(report.started_ms_ago)),
      fact("floaties", String(report.records)),
      fact("display", report.display),
    ]),
  );
  // The plugin names are a list, not a cell: a grid cell would ellipsize the one
  // name a bug report is about.
  app.body.append(row("installed plugins", report.installed_plugins.join(", ") || "none"));
  if (report.rejected_plugins.length > 0) {
    app.body.append(
      row("rejected plugins", report.rejected_plugins.join(", "), "diag-val diag-warn"),
      note(
        "A rejected folder under the plugins directory could not be loaded: its plugin.json did not parse, its API version is unknown to this build, or its index.js threw while it was imported. The log says which.",
      ),
    );
  }

  const screens = fold(
    "Screens & windows",
    `${count(report.monitors.length, "monitor")} · ${count(report.windows.length, "window")}`,
  );
  screens.body.append(el("div", "diag-subhead", "Monitors"));
  if (report.monitors.length === 0) {
    screens.body.append(note("no monitor was reported, so nothing can be placed on the desktop."));
  }
  for (const monitor of report.monitors) {
    const item = card(true);
    const head = el("div", "set-row diag-row");
    head.append(el("span", "set-label diag-key", monitor.name || monitor.key));
    if (monitor.primary) head.append(el("span", "diag-primary", "primary"));
    item.append(
      head,
      el(
        "p",
        "set-sub",
        `${monitor.x},${monitor.y} ${monitor.width}x${monitor.height} at ${scaleText(monitor.scale)}`,
      ),
    );
    screens.body.append(item);
  }

  const repairs = new Map(report.repairs.map((repair) => [repair.label, repair.attempts]));
  const beats = new Map(report.heartbeats.map((beat) => [beat.label, beat]));
  screens.body.append(el("div", "diag-subhead", "Windows"));
  if (report.windows.length === 0) {
    screens.body.append(note("floaty owns no windows right now."));
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
    const beat = beats.get(win.label);
    const attempts = repairs.get(win.label) ?? 0;
    const line = el(
      "p",
      "set-sub",
      [
        win.layer === "desktop" ? "desktop layer" : "window",
        beat ? `last beat ${ago(beat.ms_ago)} (${beat.visibility})` : "no heartbeat yet",
        attempts > 0 ? `repaired ${attempts}×` : "",
      ]
        .filter(Boolean)
        .join(" · "),
    );
    item.append(head, place, line);
    screens.body.append(item);
  }

  const disk = fold(
    "On disk",
    `store ${human(report.store.bytes)} · icons ${human(report.icons.bytes)} · log ${human(report.log.bytes)}`,
  );
  const files = card(true);
  files.append(
    row(
      "floaty-store.json",
      `${human(report.store.bytes)}, ${count(report.store.backups, "backup")}`,
    ),
    pathLine(report.store.path),
    row("icons", `${count(report.icons.files, "file")}, ${human(report.icons.bytes)}`),
    pathLine(report.icons.path),
    row("floaty.log", `${human(report.log.bytes)}, ${count(report.log.rotated, "rotated file")}`),
    pathLine(report.log.path),
  );
  disk.body.append(
    files,
    note("the log rolls at 1 MB; floaty.log.1 is the file from before the last roll."),
  );

  const tail = fold(
    "Log tail",
    `${count(report.log.lines.length, "line")} · ${human(report.log.bytes)}`,
  );
  tail.body.append(
    el(
      "pre",
      "diag-log",
      report.log.lines.length > 0 ? report.log.lines.join("\n") : "the log is empty",
    ),
  );

  host.append(presenceSection(presence), app.root, screens.root, disk.root, tail.root);
}

export const diagnosticsPane: Pane = {
  id: "diagnostics",
  label: "Diagnostics",
  title: "Diagnostics",
  hint: "The state a bug report needs: what the desktop is doing now, and the numbers behind it.",
  render(node) {
    host = node;
    void draw();
  },
};
