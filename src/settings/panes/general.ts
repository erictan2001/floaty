/**
 * The settings that are about floaty itself rather than about a widget:
 * whether it stays put under Show desktop, whether it starts with Windows,
 * whether it asks before removing something, and how to stop it.
 */

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { action, actionRow, el, group, note, toggleCard } from "../dom";
import type { Pane } from "../pane";
import { settings, updateSettings } from "../store";

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

    node.append(desktop.root, startup.root, removal.root, arrange.root, app.root);
  },
};
