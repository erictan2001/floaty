/**
 * The settings that are about floaty itself rather than about a widget:
 * whether it stays put under Show desktop, whether it asks before removing
 * something, and how to stop it.
 */

import { invoke } from "@tauri-apps/api/core";
import { action, actionRow, group, note, toggleCard } from "../dom";
import type { Pane } from "../pane";
import { settings, updateSettings } from "../store";

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

    const removal = group("Removing things");
    removal.body.append(
      toggleCard(
        "Ask before removing",
        "Ask before a floatie leaves the desktop. A file or folder floatie goes to the Recycle Bin; anything else only stops floating.",
        settings().confirm_remove,
        (on) => void updateSettings({ confirm_remove: on }, { now: true }),
      ),
    );

    const app = group("Floaties");
    const row = actionRow();
    row.append(
      action("quit floaty", () => invoke("floaty_quit"), { cls: "pill danger-btn", busyLabel: "quitting…" }),
    );
    app.body.append(
      note("Closing this window leaves floaty running in the tray. Quitting takes every floatie off the desktop until it is started again."),
      row,
    );

    node.append(desktop.root, removal.root, app.root);
  },
};
