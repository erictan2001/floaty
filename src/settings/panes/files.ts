/**
 * The root folder: which directory's contents are floated on the desktop, and
 * a way to pull them in again after something has changed on disk.
 */

import { invoke } from "@tauri-apps/api/core";
import { safe, showError } from "../api";
import { action, el, group, note } from "../dom";
import type { Pane } from "../pane";
import { reloadWidgets, settings, updateSettings } from "../store";

interface SyncResult {
  files: number;
  dirs: number;
  total: number;
}

let host: HTMLElement | undefined;

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
      setPath(picked);
      // `now`: the sync below has to see the folder the backend just stored
      await updateSettings({ files_root: picked }, { now: true });
      await runSync(picked);
    } catch {
      showError("folder picker failed");
    }
  });

  row.append(el("span", "slider-label", "root dir"), path, browse);
  row.append(action("sync", () => runSync(), { cls: "pill small ghost", busyLabel: "syncing…" }));
  section.body.append(row, count);
  section.body.append(
    note(
      "Shortcuts and programs become app floaties, everything else a file floatie (double-click opens it with its default app). Dragging an item out of a folder moves it on disk, into the desktop directory.",
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
