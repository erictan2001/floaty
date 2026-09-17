/**
 * Plugins: every kind floaty knows — the built-ins and the ones the user
 * installed — with an on/off switch, what the plugin is, and the parameters
 * that belong to that plugin alone (a pet's speed, a visualizer's gain).
 *
 * Settings that apply to every desktop item live in the Motion tab instead;
 * `getSharedRow` still hands a plugin a row for one, which is what keeps a
 * third-party plugin written against the old contract working.
 */

import { invoke } from "@tauri-apps/api/core";
import { pluginFor, type PluginSettingsContext } from "../../widgets/plugin";
import type { PluginManifestEntry } from "../../widgets/pluginManifest";
import { safe, showError } from "../api";
import { action, actionRow, card, chip, el, group, note, switchInput } from "../dom";
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

function draw(): void {
  if (!host || !host.isConnected) return;
  host.innerHTML = "";
  status = undefined;

  // The pane heading already says what this is, so the cards sit straight under
  // it rather than inside another labelled section.
  const list = el("div", "set-list");
  for (const entry of plugins()) list.append(pluginCard(entry));
  host.append(list, installGroup());
}

function pluginCard(entry: PluginManifestEntry): HTMLElement {
  const item = card(true);
  const head = el("div", "set-row");
  const origin =
    entry.source === "installed"
      ? ` — installed${entry.version ? ` v${entry.version}` : ""}${entry.author ? ` by ${entry.author}` : ""}, api v${entry.api_version}`
      : "";
  head.append(
    chip(entry.id, entry.id),
    el("span", "set-name", `${entry.name}${origin}`),
    switchInput(entry.enabled, (on, input) => {
      input.disabled = true;
      void setPluginEnabled(entry.id, on).finally(() => {
        input.disabled = false;
      });
    }),
  );
  item.append(head, note(entry.description));
  // Only an installed plugin has a folder to take away: a built-in is floaty
  // itself, and the switch above is how it is turned off.
  if (entry.source === "installed") {
    const controls = actionRow();
    controls.append(
      action("remove plugin", () => removePlugin(entry), {
        cls: "pill danger-btn",
        busyLabel: "removing…",
      }),
    );
    item.append(controls);
  }

  const plugin = pluginFor(entry.id);
  if (plugin?.renderSettings) {
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
  return item;
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
    report.relation === "downgrade"
      ? "This archive is OLDER than the copy you have."
      : "",
    report.relation === "same"
      ? "Same version as the installed copy: this rewrites it as it is."
      : "",
  ]
    .filter(Boolean)
    .join("\n\n");

  const go = await confirm(`${summary}\n\nInstall it?`, {
    title: "floaty plugins",
    kind: report.relation === "downgrade" ? "warning" : "info",
    okLabel: replaces ? "replace" : "install",
    cancelLabel: "cancel",
  });
  if (!go) return;

  const done = await safe("install plugin", () =>
    invoke<PluginInstallReport>("floaty_install_plugin", { path: picked, replace: replaces }),
  );
  if (!done) return;
  if (status) {
    status.textContent = `${done.replaced ? "replaced" : "installed"} ${done.name} v${done.version} (${done.files} files)`;
  }
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
  if (status) status.textContent = `removed ${done.id} (${done.files} files, floaties and all)`;
  await reloadPlugins();
  await reloadWidgets();
}

/** Where a third-party plugin comes from, and a way to reload the folder. */
function installGroup(): HTMLElement {
  const section = group(
    "Install your own",
    "Install a plugin .zip, or drop a folder holding plugin.json and index.js into the plugins folder and press rescan — the module is imported straight from disk. docs/PLUGINS.md describes the format.",
  );
  const row = actionRow();
  row.append(
    action("open plugin folder", () => safe("open plugin folder", () => invoke("floaty_open_plugins_dir"))),
    action("install from .zip…", installFromArchive, { busyLabel: "installing…" }),
    action(
      "rescan plugins",
      async () => {
        const rejected = await rescanPlugins();
        if (rejected === undefined) return;
        if (status) {
          status.textContent =
            rejected.length === 0 ? "all plugin folders loaded" : `rejected: ${rejected.join(" | ")}`;
        }
        await reloadPlugins();
        await reloadWidgets();
      },
      { busyLabel: "rescanning…" },
    ),
  );

  const folder = note("");
  status = note("");
  section.body.append(row, folder, status);
  void invoke<string>("floaty_plugins_dir")
    .then((dir) => {
      folder.textContent = dir;
    })
    .catch(() => undefined);
  return section.root;
}

export const pluginsPane: Pane = {
  id: "plugins",
  label: "Plugins",
  title: "Plugins",
  hint: "Everything floaty can draw on the desktop. Turning one off hides its floaties and takes its button away — nothing is deleted, and turning it back on brings them home.",
  render(node) {
    host = node;
    draw();
  },
};
