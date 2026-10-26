/**
 * The settings that are about floaty itself rather than about a widget:
 * whether it stays put under Show desktop, whether it starts with Windows,
 * whether it asks before removing something, when it gets out of the way, and
 * how to stop it.
 */

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { action, actionRow, el, group, note, textBlock, toggleCard } from "../dom";
import type { Pane } from "../pane";
import { acceleratorFrom, isModifierCode, prettyAccelerator } from "../shortcut";
import { settings, updateSettings } from "../store";

/** What the backend says about the launcher hotkey. */
interface PaletteState {
  accelerator: string;
  /** false when another app already holds that key */
  live: boolean;
  visible: boolean;
}

/** What the next undo would reverse — and the next redo would replay — as reported. */
interface UndoState {
  /** Steps left to undo, and the action the next one would reverse. */
  depth: number;
  label: string | null;
  /** Steps left to redo, and the action the next one would replay. */
  redo_depth: number;
  redo_label: string | null;
}

/** The span the backend stores for the idle timer. 0 minutes means never. */
const IDLE_MINUTES_MAX = 600;

let status: HTMLElement | undefined;
let undoButton: HTMLButtonElement | undefined;
let redoButton: HTMLButtonElement | undefined;
let watchingTidy = false;

/**
 * The undo and redo buttons say what they would do, because "undo" alone says nothing —
 * and because the two are one history seen from two places: undoing makes a redo
 * available, redoing makes the undo available again.
 */
async function refreshUndo(): Promise<void> {
  const button = undoButton;
  const redo = redoButton;
  if (!button && !redo) return;
  const state = await invoke<UndoState>("floaty_undo_state").catch(() => undefined);
  if (!state) return;
  if (button && button === undoButton) {
    button.disabled = state.depth === 0;
    button.textContent =
      state.depth === 0
        ? "nothing to undo yet"
        : state.label
          ? `undo ${state.label}`
          : `undo (${state.depth})`;
  }
  if (redo && redo === redoButton) {
    redo.disabled = (state.redo_depth ?? 0) === 0;
    redo.textContent =
      (state.redo_depth ?? 0) === 0
        ? "nothing to redo"
        : state.redo_label
          ? `redo ${state.redo_label}`
          : `redo (${state.redo_depth})`;
  }
}

/** The desktop does the tidying; this pane only asks and reports the count. */
function watchTidy(): void {
  if (watchingTidy) return;
  watchingTidy = true;
  void listen<{ count: number }>("floaty-tidy-done", (e) => {
    if (status?.isConnected) {
      status.textContent = `tidied ${e.payload?.count ?? 0} floatie(s) — Ctrl+Alt+Z, or undo below, puts them back`;
    }
  }).catch(() => undefined);
}

export const generalPane: Pane = {
  id: "general",
  label: "General",
  title: "General",
  hint: "How floaty behaves as a whole.",
  render(node) {
    node.innerHTML = "";

    const desktop = group("Desktop");
    desktop.body.append(
      toggleCard(
        "Stay on the desktop",
        "Keep floaties visible when you show the desktop (Win+D). Off: they minimise with everything else.",
        settings().stay_on_desktop,
        (on) => void updateSettings({ stay_on_desktop: on }, { now: true }),
      ),
    );

    const startup = group("Startup");
    startup.body.append(
      toggleCard(
        "Start with Windows",
        "Open floaty when you sign in, so your desktop is already there. The switch follows the real Windows startup entry: if the registry write fails, it stays where it was rather than claim something that will not happen.",
        settings().start_on_boot,
        (on) => void updateSettings({ start_on_boot: on }, { now: true }),
      ),
    );

    const removal = group("Removing things");
    removal.body.append(
      toggleCard(
        "Ask before removing",
        "Ask before a floatie leaves the desktop. A file or folder floatie goes to the Recycle Bin; anything else only stops floating.",
        settings().confirm_remove,
        (on) => void updateSettings({ confirm_remove: on }, { now: true }),
      ),
    );

    const away = group(
      "Get out of the way",
      "A fullscreen app, or an idle machine, puts the floaties aside. Neither one takes a floatie off the desktop.",
    );
    away.body.append(
      toggleCard(
        "Hide the desktop while a fullscreen app is in front",
        "The fullscreen app gets the screen to itself; the floaties return when it does not. A maximized window is not fullscreen — it leaves the taskbar showing, so floaty stays.",
        settings().hide_in_fullscreen,
        (on) => void updateSettings({ hide_in_fullscreen: on }, { now: true }),
      ),
      toggleCard(
        "Go quiet when idle",
        "No key pressed and no mouse moved anywhere on the machine for the minutes below: the motion and the microphone capture stop, and start again the moment you are back.",
        settings().quiet_when_idle,
        (on) => void updateSettings({ quiet_when_idle: on }, { now: true }),
      ),
    );
    const idleRow = el("div", "set-row");
    const idleInput = el("input", "search");
    idleInput.type = "number";
    idleInput.min = "0";
    idleInput.max = String(IDLE_MINUTES_MAX);
    idleInput.step = "1";
    idleInput.value = String(settings().idle_minutes);
    // The backend clamps this to 0..600, so write it, then show what is actually
    // in force rather than the number that was typed.
    const commitIdle = async (): Promise<void> => {
      const typed = idleInput.value.trim();
      const minutes = Number(typed);
      if (!typed || !Number.isFinite(minutes)) {
        idleInput.value = String(settings().idle_minutes);
        return;
      }
      const clamped = Math.min(IDLE_MINUTES_MAX, Math.max(0, Math.round(minutes)));
      await updateSettings({ idle_minutes: clamped }, { now: true });
      idleInput.value = String(settings().idle_minutes);
    };
    idleInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") void commitIdle();
      if (e.key === "Escape") idleInput.value = String(settings().idle_minutes);
    });
    idleInput.addEventListener("blur", () => void commitIdle());
    idleRow.append(
      textBlock(
        "Quiet after (minutes)",
        "0 means never: the floaties keep moving, and the capture keeps listening, however long the machine sits untouched.",
      ),
      idleInput,
    );
    away.body.append(idleRow);

    const launcher = group(
      "Launcher",
      "One key opens a search over your applications, the files in your pointed folder, and everything already floating on the desktop. Type, then Enter to open the top row — arrow keys move down the list, Escape closes it.",
    );
    const keyRow = el("div", "set-row");
    const keyInput = el("input", "key-input");
    keyInput.type = "text";
    keyInput.readOnly = true;
    keyInput.spellcheck = false;
    keyInput.setAttribute("aria-label", "Launcher key");
    keyInput.title = "Click, then press the combination";
    let current = settings().palette_shortcut;
    keyInput.value = prettyAccelerator(current);
    const keyStatus = el("p", "set-sub set-status");
    let listening = false;

    const describeKey = (state: PaletteState | undefined): void => {
      if (!state) {
        keyStatus.textContent = "the launcher key could not be read";
        return;
      }
      const shown = prettyAccelerator(state.accelerator);
      keyStatus.textContent = state.live
        ? `${shown} is held by floaty — press it anywhere.`
        : `${shown} is not registered: another app may already own it. The tray's “Search…” still opens the palette.`;
    };

    /** Put the field back to the key in force. */
    const stopListening = (): void => {
      listening = false;
      keyInput.classList.remove("listening");
      keyInput.value = prettyAccelerator(current);
    };

    /** Read back what the backend actually holds, rather than assuming. */
    const refreshKey = (): void => {
      void invoke<PaletteState>("floaty_palette_state")
        .then((state) => {
          current = state?.accelerator ?? current;
          if (!listening) keyInput.value = prettyAccelerator(current);
          describeKey(state);
        })
        .catch(() => describeKey(undefined));
    };

    const applyCaptured = async (accelerator: string): Promise<void> => {
      listening = false;
      keyInput.classList.remove("listening");
      keyInput.value = prettyAccelerator(accelerator);
      await updateSettings({ palette_shortcut: accelerator }, { now: true });
      const state = await invoke<PaletteState>("floaty_palette_state").catch(() => undefined);
      current = state?.accelerator ?? settings().palette_shortcut;
      keyInput.value = prettyAccelerator(current);
      describeKey(state);
      keyInput.blur();
    };

    keyInput.addEventListener("focus", () => {
      listening = true;
      keyInput.classList.add("listening");
      keyInput.value = "press a key…";
      keyStatus.textContent = "hold Ctrl, Alt or Win with another key — Esc cancels.";
    });
    keyInput.addEventListener("blur", () => {
      if (!listening) return;
      stopListening();
      refreshKey();
    });
    keyInput.addEventListener("keydown", (e) => {
      if (!listening) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        stopListening();
        refreshKey();
        keyInput.blur();
        return;
      }
      // Tab and Shift+Tab leave the field. Capturing them would trap focus.
      if (e.code === "Tab" && !e.ctrlKey && !e.altKey && !e.metaKey) {
        stopListening();
        refreshKey();
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      if (isModifierCode(e.code)) {
        // A modifier on its own is not the shortcut yet: show what is held.
        const held = [
          e.ctrlKey ? "Ctrl" : "",
          e.altKey ? "Alt" : "",
          e.shiftKey ? "Shift" : "",
          e.metaKey ? "Win" : "",
        ].filter(Boolean);
        keyInput.value = held.length ? `${held.join(" + ")} + …` : "press a key…";
        return;
      }
      const captured = acceleratorFrom({
        code: e.code,
        ctrl: e.ctrlKey,
        alt: e.altKey,
        shift: e.shiftKey,
        meta: e.metaKey,
      });
      if (!captured.ok) {
        keyStatus.textContent =
          captured.reason === "modifier"
            ? "hold Ctrl, Alt or Win as well — a bare key would take that key from every app."
            : "that key cannot be the launcher key; try a letter, number, F1–F12 or Space.";
        keyInput.value = "press a key…";
        return;
      }
      void applyCaptured(captured.accelerator);
    });
    keyRow.append(
      textBlock("Launcher key", "Click, then press the keys — hold Ctrl, Alt or Win."),
      keyInput,
    );
    refreshKey();
    const launcherRow = actionRow();
    launcherRow.append(
      action("show the launcher", () => invoke("floaty_show_palette"), { cls: "pill ghost" }),
    );
    launcher.body.append(keyRow, keyStatus, launcherRow);

    const arrange = group(
      "Arrange and undo",
      "Tidy lines the icons up on the grid — pinned ones in rows from the top, the rest stacked up from the floor, which is where gravity would leave them anyway. Panels and notes keep their places. **Ctrl+Alt+Z** from anywhere undoes the last change: a moved icon, a grouping, an ungroup, a tidy, a recycled file.",
    );
    watchTidy();
    const arrangeRow = actionRow();
    const undo = el("button", "pill", "undo");
    undo.addEventListener("click", () => {
      undo.disabled = true;
      void invoke<{ label: string }>("floaty_undo")
        .then((report) => {
          if (status?.isConnected) status.textContent = `undid '${report.label}'`;
        })
        .catch((err) => {
          if (status?.isConnected) status.textContent = String(err);
        })
        .finally(() => void refreshUndo());
    });
    undoButton = undo;
    const redo = el("button", "pill", "redo");
    redo.addEventListener("click", () => {
      redo.disabled = true;
      void invoke<{ label: string }>("floaty_redo")
        .then((report) => {
          if (status?.isConnected) status.textContent = `redid '${report.label}'`;
        })
        .catch((err) => {
          if (status?.isConnected) status.textContent = String(err);
        })
        .finally(() => void refreshUndo());
    });
    redoButton = redo;
    void refreshUndo();
    arrangeRow.append(
      action("tidy the desktop", () => emit("floaty-tidy-requested"), {
        busyLabel: "tidying…",
      }),
      undo,
      redo,
    );
    status = note("");
    arrange.body.append(arrangeRow, status);

    const updates = group("Updates");
    const updateRow = actionRow();
    const updateStatus = note("");
    const install = action(
      "install and restart",
      async () => {
        try {
          await invoke("floaty_install_update");
        } catch (err) {
          if (updateStatus?.isConnected) updateStatus.textContent = String(err);
        }
      },
      { busyLabel: "installing…" },
    );
    install.disabled = true;
    updateRow.append(
      action(
        "check for updates",
        async () => {
          try {
            const report = await invoke<{
              available: boolean;
              current: string;
              version?: string | null;
              notes?: string | null;
              error?: string | null;
            }>("floaty_check_update");
            if (!updateStatus?.isConnected) return;
            if (report.error) {
              // A check that could not be made is not "up to date", and saying so is the
              // difference between a quiet failure and a wrong claim.
              updateStatus.textContent = `could not check: ${report.error}`;
              install.disabled = true;
            } else if (report.available) {
              updateStatus.textContent = `${report.version} is available — you have ${report.current}${
                report.notes ? `: ${report.notes}` : ""
              }`;
              install.disabled = false;
            } else {
              updateStatus.textContent = `${report.current} is the newest version`;
              install.disabled = true;
            }
          } catch (err) {
            if (updateStatus?.isConnected) updateStatus.textContent = String(err);
          }
        },
        { busyLabel: "checking…" },
      ),
      install,
    );
    updates.body.append(
      note(
        "Updates come from this project's releases and are signed: the installer is refused unless the signature matches the public key built into this copy. Nothing downloads or installs unless you ask.",
      ),
      updateRow,
      updateStatus,
    );

    const app = group("Floaties");
    const row = actionRow();
    row.append(
      action("quit floaty", () => invoke("floaty_quit"), {
        cls: "pill danger-btn",
        busyLabel: "quitting…",
      }),
    );
    app.body.append(
      note(
        "Closing this window leaves floaty running in the tray. Quitting takes every floatie off the desktop until it is started again.",
      ),
      row,
    );

    node.append(
      desktop.root,
      startup.root,
      removal.root,
      away.root,
      launcher.root,
      arrange.root,
      updates.root,
      app.root,
    );
  },
};
