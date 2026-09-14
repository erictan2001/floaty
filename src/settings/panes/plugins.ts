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
      ? ` — installed${entry.version ? ` v${entry.version}` : ""}${entry.author ? ` by ${entry.author}` : ""}`
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

/** Where a third-party plugin comes from, and a way to reload the folder. */
function installGroup(): HTMLElement {
  const section = group(
    "Install your own",
    "Drop a folder holding plugin.json and index.js into the plugins folder, then press rescan — the module is imported straight from disk. docs/PLUGINS.md describes the format.",
  );
  const row = actionRow();
  row.append(
    action("open plugin folder", () => safe("open plugin folder", () => invoke("floaty_open_plugins_dir"))),
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
