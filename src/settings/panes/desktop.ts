/**
 * The floaties that are on the desktop right now: one button per plugin to add
 * one, then a row per floatie with the plugin's own controls and a way out.
 */

import { pluginFor } from "../../widgets/plugin";
import { manifestEntries } from "../../widgets/pluginManifest";
import { removeSelf, type WidgetRecord } from "../../widgets/lib";
import { safe, showError } from "../api";
import { action, actionRow, card, chip, el, emptyState, group, note } from "../dom";
import type { Pane } from "../pane";
import { addFloatie, disabledKinds, reloadWidgets, widgets } from "../store";

let host: HTMLElement | undefined;

function draw(): void {
  if (!host || !host.isConnected) return;
  host.innerHTML = "";
  host.append(addGroup(), listGroup());
}

/** Quick add: a button per plugin that offers one, straight from the manifest. */
function addGroup(): HTMLElement {
  const off = disabledKinds();
  const group_ = group("New floatie", "A button per plugin that can make one.");
  // `set-add` softens the pills: filled violet is the tab bar's language, and
  // five of them next to six tabs read as a second row of navigation.
  const row = actionRow();
  row.classList.add("set-add");
  const entries = manifestEntries().filter((entry) => entry.add_label && !off.has(entry.id));
  if (entries.length === 0) {
    row.append(emptyState("every plugin is off — turn one on in Plugins"));
  }
  for (const entry of entries) {
    row.append(
      action(entry.add_label!, async () => {
        const rec = await addFloatie(entry.id);
        if (rec) await reloadWidgets();
      }),
    );
  }
  group_.body.append(row);
  return group_.root;
}

function listGroup(): HTMLElement {
  const off = disabledKinds();
  const visible = widgets().filter((rec) => !off.has(rec.kind));
  const hidden = widgets().length - visible.length;
  const group_ = group(
    visible.length ? `On your desktop (${visible.length})` : "On your desktop",
    "Right-click a floatie on the desktop to open it, rename it, pin it on top or delete it.",
  );
  if (visible.length === 0) {
    group_.body.append(emptyState("no floaties yet — add one above, or use the tray icon"));
  }
  for (const rec of [...visible].sort((a, b) => a.id.localeCompare(b.id))) {
    group_.body.append(floatieCard(rec));
  }
  if (hidden > 0) {
    group_.body.append(note(`${hidden} floatie${hidden === 1 ? "" : "s"} hidden — their plugin is off.`));
  }
  return group_.root;
}

function floatieCard(rec: WidgetRecord): HTMLElement {
  const item = card();
  item.append(chip(rec.kind, rec.kind));
  const plugin = pluginFor(rec.kind);
  const detail = plugin?.describe?.(rec);
  item.append(el("span", "set-name", detail ? `${rec.id} — ${detail}` : rec.id));

  // The remove button goes in before the plugin's own controls: it is the
  // reference the plugin slots its control in front of (see the plugin docs).
  const remove = el("button", "danger", "remove");
  remove.addEventListener("click", async () => {
    if (remove.disabled) return;
    remove.disabled = true;
    try {
      // `removeSelf` is the one removal path — it asks first when the user wants
      // to be asked, and recycles a file/folder floatie instead of only dropping
      // its record, so the desktop and the disk stay in step.
      await removeSelf(rec);
      await reloadWidgets();
    } finally {
      remove.disabled = false;
    }
  });
  item.append(remove);

  if (plugin?.renderWidgetControls) {
    void plugin.renderWidgetControls(
      item,
      rec,
      { safe, refreshWidgets: reloadWidgets, showError },
      remove,
    );
  }
  return item;
}

export const desktopPane: Pane = {
  id: "desktop",
  label: "Desktop",
  title: "Your desktop",
  hint: "What is floating right now, and everything that can be added without pointing at a folder.",
  render(node) {
    host = node;
    draw();
  },
};
