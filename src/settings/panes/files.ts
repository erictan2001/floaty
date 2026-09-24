/**
 * The root folder: which directory's contents are floated on the desktop, and
 * a way to pull them in again after something has changed on disk.
 */

import { invoke } from "@tauri-apps/api/core";
import { safe, showError } from "../api";
import { action, card, el, group, note, textBlock } from "../dom";
import type { Pane } from "../pane";
import { reloadWidgets, settings, updateSettings } from "../store";

interface SyncResult {
  files: number;
  dirs: number;
  total: number;
}

let host: HTMLElement | undefined;

/**
 * What the last root change meant, said once, in the pane that made it.
 *
 * Changing the root is a change the user has already made, so it is *not* asked
 * about: the first run blocked for an answer, and every change after that warns and
 * proceeds, because a confirmation step the user has learned to click through is
 * worse than no step at all (ADR 0004). Kept at module scope because a settings
 * change redraws the pane — a warning that vanished the moment the write landed
 * would be no warning at all.
 */
let warning = "";

/** Windows paths, compared the way the filesystem does. */
function samePath(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

function draw(): void {
  if (!host || !host.isConnected) return;
  host.innerHTML = "";

  const section = group("Root folder");
  const row = el("div", "slider-row");
  let rootPath = settings().files_root;

  const path = el("span", "folder-path", rootPath || "no folder set");
  path.title = rootPath;
  const count = el("div", "count", "");

  const setPath = (value: string): void => {
    rootPath = value;
    path.textContent = value || "no folder set";
    path.title = value;
  };

  async function runSync(target?: string): Promise<void> {
    const root = target ?? rootPath;
    if (!root) {
      count.textContent = "no root folder set";
      return;
    }
    const result = await safe("sync files", () => invoke<SyncResult>("floaty_sync_files", { root }));
    if (!result) return;
    const files = `${result.files} file${result.files === 1 ? "" : "s"}`;
    const dirs = `${result.dirs} folder${result.dirs === 1 ? "" : "s"}`;
    count.textContent = `${files}, ${dirs} synced to the desktop`;
    await reloadWidgets();
  }

  const browse = action("browse", async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked !== "string" || !picked) return;
      const previous = rootPath;
      setPath(picked);
      // `now`: the sync below has to see the folder the backend just stored
      await updateSettings({ files_root: picked }, { now: true });
      // A change after the first answer is a change, not a first adoption: it warns
      // and proceeds. Before the first answer the guard is the one that shows this
      // folder and waits, so there is nothing to add here.
      if (settings().root_confirmed && previous && !samePath(previous, picked)) {
        warning =
          `The desktop now follows ${picked}. The floaties from ${previous} came off the ` +
          `desktop — their files were not touched and are still in ${previous}.`;
      }
      await runSync(picked);
    } catch {
      showError("folder picker failed");
    }
  });

  row.append(el("span", "slider-label", "root dir"), path, browse);
  row.append(action("sync", () => runSync(), { cls: "pill small ghost", busyLabel: "syncing…" }));
  section.body.append(row, count);

  if (warning) {
    const said = card(true);
    const line = el("div", "set-row");
    line.append(textBlock("the root changed", warning));
    said.append(
      line,
      action(
        "dismiss",
        () => {
          warning = "";
          draw();
        },
        { cls: "pill small ghost" },
      ),
    );
    section.body.append(said);
  }

  section.body.append(
    note(
      "Shortcuts and programs become app floaties, everything else a file floatie (double-click opens it with its default app). Dragging an item out of a folder moves it on disk, into the desktop directory. Pointing floaty at a different folder takes the old folder's floaties off the desktop — their files stay where they are.",
    ),
  );
  host.append(section.root);
}

export const filesPane: Pane = {
  id: "files",
  label: "Files",
  title: "Files & folders",
  hint: "Point floaty at a folder and its contents land on the desktop, as floaties that know where they came from.",
  render(node) {
    host = node;
    draw();
  },
};
