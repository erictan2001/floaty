import { invoke } from "@tauri-apps/api/core";
import { desktopItemFor, pathOf } from "./pluginManifest";
import { appWin, WidgetRecord } from "./windowState";
import { currentSettings } from "./theme";

export async function loadRecord(id: string): Promise<WidgetRecord | undefined> {
  try {
    return await invoke<WidgetRecord | null>("floaty_get_record", { id }).then(
      (r) => r ?? undefined,
    );
  } catch (err) {
    void invoke("floaty_log", {
      msg: `[lib] loadRecord(${id}) failed: ${String(err)}`,
    }).catch(() => undefined);
    return undefined;
  }
}

export async function saveRecord(rec: WidgetRecord): Promise<void> {
  try {
    await invoke("floaty_save", { record: rec });
  } catch (err) {
    void invoke("floaty_log", {
      msg: `[lib] saveRecord(${rec.id}) failed: ${String(err)}`,
    }).catch(() => undefined);
  }
}

export function makeBar(title: string, onClose: () => void): HTMLElement {
  const bar = document.createElement("div");
  bar.className = "bar";
  const label = document.createElement("span");
  label.className = "bar-title";
  label.textContent = title;
  const close = document.createElement("button");
  close.className = "icon-btn";
  close.textContent = "\u00d7";
  close.title = "Remove widget";
  close.addEventListener("pointerdown", (e) => e.stopPropagation());
  close.addEventListener("click", (e) => {
    e.stopPropagation();
    onClose();
  });
  bar.append(label, close);
  return bar;
}

export async function confirmRemoveDialog(what: string, kind: string): Promise<boolean> {
  const item = desktopItemFor(kind);
  const onDisk = item !== undefined;
  const noun = item?.noun ?? `${kind} floatie`;
  const body = onDisk
    ? `Delete this ${noun}?\n\n${what}\n\nIf it lives on your desktop it goes to the Recycle Bin (recoverable); anything outside the desktop only loses its floatie.`
    : `Remove this ${noun} from the desktop?\n\n${what}`;
  try {
    const { confirm } = await import("@tauri-apps/plugin-dialog");
    return await confirm(body, {
      title: "floaty",
      kind: "warning",
      okLabel: "remove",
      cancelLabel: "keep",
    });
  } catch {
    return window.confirm(body);
  }
}

const LAUNCH_SUFFIX = /\.(lnk|url|exe|bat|cmd|com|msi|appref-ms|ps1|scr|jar)$/i;

export function displayName(name: string): string {
  const stripped = name.replace(LAUNCH_SUFFIX, "");
  return stripped || name;
}

export function describeForConfirm(rec: WidgetRecord): string {
  const raw =
    typeof rec.data["name"] === "string" && (rec.data["name"] as string)
      ? (rec.data["name"] as string)
      : rec.id;
  const name = displayName(raw);
  const target = pathOf(rec);
  return target ? `${name}\n${target}` : name;
}

export function hasDesktopPath(rec: WidgetRecord): boolean {
  return pathOf(rec) !== "";
}

export async function removeSelf(rec: WidgetRecord): Promise<void> {
  if (
    currentSettings().confirm_remove &&
    !(await confirmRemoveDialog(describeForConfirm(rec), rec.kind))
  ) {
    return;
  }
  try {
    if (hasDesktopPath(rec)) {
      await invoke<string>("floaty_delete", { id: rec.id });
    } else {
      await invoke("floaty_remove", { id: rec.id });
    }
  } catch (err) {
    void invoke("floaty_log", {
      msg: `[self] removing ${rec.id} (${rec.kind}) failed: ${String(err)}`,
    }).catch(() => undefined);
    if (!appWin.label.startsWith("widget-")) return;
    await appWin.close().catch(() => undefined);
  }
}
