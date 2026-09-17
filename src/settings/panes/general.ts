/**
 * The settings that are about floaty itself rather than about a widget:
 * whether it stays put under Show desktop, whether it starts with Windows,
 * whether it asks before removing something, and how to stop it.
 */

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { action, actionRow, el, group, note, textBlock, toggleCard } from "../dom";
import type { Pane } from "../pane";
import { settings, updateSettings } from "../store";

/** What the backend says about the launcher hotkey. */
interface PaletteState {
  accelerator: string;
  /** false when another app already holds that key */
  live: boolean;
  visible: boolean;
}

/** What the next undo would reverse, as the backend reports it. */
interface UndoState {
  depth: number;
  label: string | null;
}

let status: HTMLElement | undefined;
let undoButton: HTMLButtonElement | undefined;
let watchingTidy = false;

/** The undo button says what it would undo, because "undo" alone says nothing. */
async function refreshUndo(): Promise<void> {
  const button = undoButton;
  if (!button) return;
  const state = await invoke<UndoState>("floaty_undo_state").catch(() => undefined);
  if (!state || button !== undoButton) return;
  button.disabled = state.depth === 0;
  button.textContent = state.depth === 0
    ? "nothing to undo yet"
    : state.label
      ? `undo ${state.label}`
      : `undo (${state.depth})`;
}

/** The desktop does the tidying; this pane only asks and reports the count. */
function watchTidy(): void {
  if (watchingTidy) return;
  watchingTidy = true;
  void listen<{ count: number }>("floaty-tidy-done", (e) => {
    if (status?.isConnected) {
      status.textContent = `tidied ${e.payload?.count ?? 0} floatie(s) — Ctrl+Z, or undo below, puts them back`;
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

    const launcher = group(
      "Launcher",
      "One key opens a search over your applications, the files in your pointed folder, and everything already floating on the desktop. Type, then Enter to open the top row — arrow keys move down the list, Escape closes it.",
    );
    const keyRow = el("div", "set-row");
    const keyInput = el("input", "search");
    keyInput.type = "text";
    keyInput.spellcheck = false;
    keyInput.value = settings().palette_shortcut;
    keyInput.placeholder = "Ctrl+Alt+Space";
    const keyStatus = note("");
    const describeKey = (state: PaletteState | undefined): void => {
      if (!state) {
        keyStatus.textContent = "the launcher key could not be read";
        return;
      }
      keyStatus.textContent = state.live
        ? `${state.accelerator} is held by floaty — press it anywhere.`
        : `${state.accelerator} is not registered: another app may already own it. The tray's “Search…” still opens the palette.`;
    };
    const commitKey = async (): Promise<void> => {
      const wanted = keyInput.value.trim();
      if (!wanted) {
        keyInput.value = settings().palette_shortcut;
        return;
      }
      // The backend keeps the working key when a combination does not parse, so
      // read back what is actually in force rather than assuming.
      await updateSettings({ palette_shortcut: wanted }, { now: true });
      const state = await invoke<PaletteState>("floaty_palette_state").catch(() => undefined);
      keyInput.value = state?.accelerator ?? settings().palette_shortcut;
      describeKey(state);
    };
    keyInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") void commitKey();
      if (e.key === "Escape") keyInput.value = settings().palette_shortcut;
    });
    keyInput.addEventListener("blur", () => void commitKey());
    keyRow.append(textBlock("Launcher key", "A key combination, like Ctrl+Alt+Space or Win+Shift+K."), keyInput);
    void invoke<PaletteState>("floaty_palette_state")
      .then(describeKey)
      .catch(() => describeKey(undefined));
    const launcherRow = actionRow();
    launcherRow.append(
      action("show the launcher", () => invoke("floaty_show_palette")),
    );
    launcher.body.append(keyRow, keyStatus, launcherRow);

    const arrange = group(
      "Arrange and undo",
      "Tidy lines the icons up on the grid — pinned ones in rows from the top, the rest stacked up from the floor, which is where gravity would leave them anyway. Panels and notes keep their places. Ctrl+Z on the desktop undoes the last change: a recycled file, an ungroup, a grouping, a tidy.",
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
    void refreshUndo();
    arrangeRow.append(
      action("tidy the desktop", () => emit("floaty-tidy-requested"), {
        busyLabel: "tidying…",
      }),
      undo,
    );
    status = note("");
    arrange.body.append(arrangeRow, status);

    const app = group("Floaties");
    const row = actionRow();
    row.append(
      action("quit floaty", () => invoke("floaty_quit"), { cls: "pill danger-btn", busyLabel: "quitting…" }),
    );
    app.body.append(
      note("Closing this window leaves floaty running in the tray. Quitting takes every floatie off the desktop until it is started again."),
      row,
    );

    node.append(desktop.root, startup.root, removal.root, launcher.root, arrange.root, app.root);
  },
};
