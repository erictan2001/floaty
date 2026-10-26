/**
 * Plugins: every kind floaty knows, in two short lists — the built-ins, then the
 * ones the user installed — with an on/off switch and the settings that belong
 * to that plugin alone (a visualizer's gain, the folder Live2D scans).
 *
 * The kind badge is not repeated here: the list is grouped by origin already, and
 * a plugin's name is what a person looks for. Settings that apply to every
 * desktop item live in Motion; `getSharedRow` still hands a plugin a row for one.
 */

import { invoke } from "@tauri-apps/api/core";
import { pluginFor, type PluginSettingsContext } from "../../widgets/plugin";
import type { PluginManifestEntry } from "../../widgets/pluginManifest";
import { safe, showError } from "../api";
import { action, actionRow, card, el, emptyState, group, switchInput, textBlock } from "../dom";
import type { Pane } from "../pane";
import { sharedRow } from "../params";
import {
  plugins,
  reloadPlugins,
  reloadWidgets,
  rescanPlugins,
  setPluginEnabled,
  settings,
  updateSettings,
  widgets,
} from "../store";

let host: HTMLElement | undefined;
let status: HTMLElement | undefined;
/** The last action's news. Kept across a redraw, because reloading the list is
 *  exactly what the action does — a line that vanished with the reload was a
 *  line nobody ever read. */
let message = "";

function say(text: string): void {
  message = text;
  if (status?.isConnected) status.textContent = text;
}

function draw(): void {
  if (!host || !host.isConnected) return;
  host.innerHTML = "";
  const all = plugins();
  host.append(
    tools(),
    statusLine(),
    pluginGroup(
      "Built in",
      all.filter((entry) => entry.source !== "installed"),
    ),
    pluginGroup(
      "Installed",
      all.filter((entry) => entry.source === "installed"),
      "Plugins you added. Removing one recycles its folder; its floaties go with it.",
    ),
  );
}

/** Install, open the folder, rescan: three buttons, no section of their own. */
function tools(): HTMLElement {
  const row = actionRow();
  row.classList.add("set-toolbar");
  row.append(
    action("install .zip…", installFromArchive, { busyLabel: "installing…" }),
    action(
      "open plugin folder",
      () => safe("open plugin folder", () => invoke("floaty_open_plugins_dir")),
      { cls: "pill ghost" },
    ),
    action(
      "rescan",
      async () => {
        const rejected = await rescanPlugins();
        if (rejected === undefined) return;
        say(
          rejected.length === 0 ? "all plugin folders loaded" : `rejected: ${rejected.join(" | ")}`,
        );
        await reloadPlugins();
        await reloadWidgets();
      },
      { cls: "pill ghost", busyLabel: "rescanning…" },
    ),
  );
  return row;
}

function statusLine(): HTMLElement {
  status = el("p", "set-sub set-status", message);
  return status;
}

function pluginGroup(
  title: string,
  entries: PluginManifestEntry[],
  noteText?: string,
): HTMLElement {
  const section = group(entries.length > 0 ? `${title} (${entries.length})` : title, noteText);
  if (entries.length === 0) {
    section.body.append(emptyState("nothing here yet"));
    return section.root;
  }
  for (const entry of entries) section.body.append(pluginCard(entry));
  return section.root;
}

function pluginCard(entry: PluginManifestEntry): HTMLElement {
  const item = card(true);
  const head = el("div", "set-row");
  const meta =
    entry.source === "installed"
      ? [
          entry.version ? `v${entry.version}` : "",
          entry.author ? `by ${entry.author}` : "",
          `API ${entry.api_version}`,
        ]
          .filter(Boolean)
          .join(" · ")
      : "";
  const text = textBlock(entry.name, entry.description);
  if (meta) text.append(el("span", "set-meta", meta));
  head.append(
    text,
    switchInput(entry.enabled, (on, input) => {
      input.disabled = true;
      void setPluginEnabled(entry.id, on).finally(() => {
        input.disabled = false;
      });
    }),
  );
  item.append(head);

  // Only an installed plugin has code to trust and a folder to take away: a
  // built-in is floaty itself, and the switch above is how it is turned off.
  if (entry.source === "installed") {
    if (!entry.trusted) {
      item.append(
        el(
          "p",
          "set-sub set-warn",
          "Not approved — its floaties will not mount until you approve it.",
        ),
      );
    }
    const plugin = pluginFor(entry.id);
    addPluginSettings(item, plugin);

    const controls = actionRow();
    controls.append(
      action(
        entry.trusted ? "withdraw approval" : "approve plugin",
        () => setPluginTrusted(entry, !entry.trusted),
        { cls: "pill ghost", busyLabel: entry.trusted ? "withdrawing…" : "approving…" },
      ),
      action("remove plugin", () => removePlugin(entry), {
        cls: "pill danger-btn",
        busyLabel: "removing…",
      }),
    );
    item.append(controls);
    return item;
  }

  addPluginSettings(item, pluginFor(entry.id));
  return item;
}

/** A plugin's own controls, when it has any. */
function addPluginSettings(item: HTMLElement, plugin: ReturnType<typeof pluginFor>): void {
  if (!plugin?.renderSettings) return;
  const ctx: PluginSettingsContext = {
    getSettings: settings,
    getSharedRow: sharedRow,
    safe,
    showError,
    refreshWidgets: reloadWidgets,
    updateSettings,
  };
  void plugin.renderSettings(item, ctx);
}

/**
 * Approve an installed plugin's code, or take the approval back.
 *
 * The backend records the fingerprint of the folder as it is now; withdrawing
 * forgets it, so the widgets stop mounting (they are not removed — approving again
 * brings them back).
 */
async function setPluginTrusted(entry: PluginManifestEntry, trusted: boolean): Promise<void> {
  const done = await safe("set plugin approval", () =>
    invoke<null>("floaty_plugin_trust", { id: entry.id, trusted }),
  );
  if (done === undefined) return;
  say(trusted ? `approved ${entry.id}` : `approval withdrawn for ${entry.id}`);
  await reloadPlugins();
  await reloadWidgets();
}

/** What an archive holds, as the backend reads it before installing anything. */
interface ArchiveReport {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string;
  api_version: number;
  add_label: string | null;
  files: number;
  size_bytes: number;
  installed_version: string | null;
  /** new | same | upgrade | downgrade | unknown */
  relation: string;
}

/** What the backend reports about one install. */
interface PluginInstallReport {
  id: string;
  name: string;
  version: string;
  files: number;
  replaced: boolean;
}

/** What the backend reports about one removal. */
interface PluginUninstallReport {
  id: string;
  files: number;
  recycled_to: string;
}

/**
 * Install a plugin from a zip the user picked: read it, say what it is, then do it.
 *
 * The confirm is built from the archive itself, by the same code that would install
 * it, so what is offered and what lands cannot disagree — and an archive that is
 * older than the installed copy says so, because that is almost always not what
 * somebody meant.
 */
async function installFromArchive(): Promise<void> {
  const { open, confirm } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    multiple: false,
    title: "a plugin archive",
    filters: [{ name: "plugin archive", extensions: ["zip"] }],
  });
  if (typeof picked !== "string" || !picked) return;

  const report = await safe("read the plugin archive", () =>
    invoke<ArchiveReport>("floaty_inspect_plugin", { path: picked }),
  );
  if (!report) return;

  const replaces = report.installed_version !== null;
  const summary = [
    `${report.name} v${report.version || "?"}  (${report.id})`,
    report.author ? `by ${report.author}` : "",
    report.description,
    `${report.files} file(s), ${Math.max(1, Math.round(report.size_bytes / 1024))} KB, plugin api v${report.api_version}`,
    replaces
      ? `replaces the installed v${report.installed_version} — ${report.relation}`
      : "not installed yet",
    report.relation === "downgrade" ? "This archive is OLDER than the copy you have." : "",
    report.relation === "same"
      ? "Same version as the installed copy: this rewrites it as it is."
      : "",
  ]
    .filter(Boolean)
    .join("\n\n");

  // Installing from an archive *is* the approval: this dialog is where the user
  // sees what the code is, who wrote it and what it replaces. The backend records
  // it, so the plain fact is worth a line here.
  const go = await confirm(
    `${summary}\n\nInstalling approves this plugin's code for your desktop. The approval is tied to these files: replacing them later asks again.`,
    {
      title: "floaty plugins",
      kind: report.relation === "downgrade" ? "warning" : "info",
      okLabel: replaces ? "replace" : "install",
      cancelLabel: "cancel",
    },
  );
  if (!go) return;

  const done = await safe("install plugin", () =>
    invoke<PluginInstallReport>("floaty_install_plugin", { path: picked, replace: replaces }),
  );
  if (!done) return;
  say(
    `${done.replaced ? "replaced" : "installed"} ${done.name} v${done.version} (${done.files} files)`,
  );
  await reloadPlugins();
  await reloadWidgets();
}

/**
 * Take a plugin off the desktop.
 *
 * Its floaties have to go with it — a record whose kind is no longer in the
 * manifest is drawn by a build that has no idea what it is — so the count is part
 * of the question rather than a surprise afterwards. The folder goes to the
 * Recycle Bin, so the answer is reversible.
 */
async function removePlugin(entry: PluginManifestEntry): Promise<void> {
  const { confirm } = await import("@tauri-apps/plugin-dialog");
  const mine = widgets().filter((rec) => rec.kind === entry.id);
  const body = [
    `Remove the plugin "${entry.name}" (${entry.id} v${entry.version || "?"})?`,
    mine.length
      ? `Its ${mine.length} floatie(s) on the desktop go with it.`
      : "Nothing of it is on the desktop.",
    "The plugin folder goes to the Recycle Bin, so it can be restored from there.",
  ].join("\n\n");
  const yes = await confirm(body, {
    title: "floaty plugins",
    kind: "warning",
    okLabel: "remove",
    cancelLabel: "keep",
  });
  if (!yes) return;

  const done = await safe("remove plugin", () =>
    invoke<PluginUninstallReport>("floaty_uninstall_plugin", {
      id: entry.id,
      removeWidgets: mine.length > 0,
    }),
  );
  if (!done) return;
  say(`removed ${done.id} (${done.files} files, floaties and all)`);
  await reloadPlugins();
  await reloadWidgets();
}

export const pluginsPane: Pane = {
  id: "plugins",
  label: "Plugins",
  title: "Plugins",
  hint: "Turning one off hides its floaties and its add button. Nothing is deleted.",
  render(node) {
    host = node;
    draw();
  },
};
