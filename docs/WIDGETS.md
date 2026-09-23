# Widgets, plugins and settings

What you can put on the desktop besides your files, how to write one of your
own, and where every setting and file lives.

## Widgets

Built in (`src-tauri/src/plugins.rs` is the manifest):

| Kind | What it is |
| --- | --- |
| `app` | gravity launcher for a real app; icons group into folders |
| `file` | a loose file in the root; double-click opens it with its default app |
| `folder` | a group of launchers; click to expand into a launch grid |
| `note` | sticky note, drag by the top bar, text autosaves |
| `clock` | clock plus pomodoro timer |
| `live2d` | animated Live2D companion (pixi + Cubism 2/3/4) |
| `visualizer` | spectrum bars for system audio (loopback, not the mic) |
| `sysmon` | CPU, 3D GPU and RAM load with a scrolling graph |

`countdown` and `trail`, in [`examples/plugins`](../examples/plugins), are two
complete plugins written the third-party way — one that keeps to itself, one that
rearranges the desktop — and both are meant to be read. Their
[README](../examples/plugins/README.md) says how to install them and what each one
is worth reading for.

## Plugins

Every floatie is a plugin, described twice on purpose: the backend manifest owns
names, sizes, the quick-add label and whether the kind stands for a file on disk,
while the frontend module owns behaviour — mount, describe, settings rows. The
frontend reads the manifest, so those facts exist once.

Third-party plugins need no Floaty source: press **install from .zip** with a
plugin archive somebody sent you, or drop a folder with `plugin.json` and
`index.js` into `%APPDATA%\com.floaty.app\plugins` and press **rescan plugins**.
Either way the widget is available with the same API and the same settings card as
a built-in. Installation, the `plugin.json` reference and the module contract are in
[docs/PLUGINS.md](PLUGINS.md).

An archive is read *before* it is installed: the confirm names the plugin, its
version, its author, how much of it there is, which plugin api it was written
against, and — if you already have it — both versions and whether this is an
upgrade or a downgrade. **Installing is approving**: a plugin's code runs inside
Floaty's own windows, so an installed plugin is not loaded until it has been
approved, and the approval is recorded against a fingerprint of the files it was
given for — replace them and Floaty asks again. A folder copied into the plugins
directory by hand is listed with an **approve plugin** button and stays unloaded
until you press it. Anything that is not a plugin, or is not a zip at all, is
refused by name with the reason, with nothing written to the plugins folder. The
**remove plugin** button on a plugin's card takes it off again: the folder goes to
the Recycle Bin and its floaties go with it, and it says how many that is first.

Disabling a plugin closes its widgets and keeps their records; re-enabling brings
them back. Nothing disabled is loaded, listed or creatable until it is turned back
on.

## Settings window

Tray icon → **Settings** (the tray is Settings and Quit). One tab per question you
arrive with, and the tab lives in the URL hash so a reload stays where you were:

| Tab | Hash | Holds |
| --- | --- | --- |
| Desktop | `#/desktop` | what is on your desktop, removal |
| Apps | `#/apps` | scan and float applications |
| Files | `#/files` | the root folder and its contents |
| Motion | `#/motion` | the table above, plus click behaviour |
| Plugins | `#/plugins` | per-plugin toggles, parameter cards, rescan |
| General | `#/general` | stay on desktop, start with Windows, ask before removing, the launcher key, tidy the desktop, undo, check for updates |
| Diagnostics | `#/diagnostics` | `floaty.log`, the monitors, every window, the heartbeat table, the sizes on disk |

Edits are applied immediately; slider drags are debounced so a five-step drag is
one save.

Icons are files, not text in the store. A record's `icon` is an
`http://asset.localhost/…` url into that folder, which is what an `<img>` loads
either way — so the desktop renders them exactly as it did when they were inlined
base64. Measured on a real desktop of 36 widgets: 451 stored icons (8.39MB, 97% of
an 8.65MB store) became 42 files totalling 1.8MB, and the store became 300KB.
Identical bytes are one file, which is why ten folders holding the same app icon
cost one icon. A store written before this change is migrated on the next launch
(`icons: moved N inlined icons into …`), and icon files no record mentions any
more are swept.

## Where things live

| Path | Contents |
| --- | --- |
| `%APPDATA%\com.floaty.app\floaty-store.json` | widget records: `id`, `kind`, logical `x`/`y`, and the per-widget `data` (name, target, `pinned`, and an `icon` url) |
| `%APPDATA%\com.floaty.app\icons\` | one PNG per *distinct* icon, named after its contents, so the same app icon in ten folders is one file |
| `%APPDATA%\com.floaty.app\floaty-settings.json` | global settings (the keys in the Motion table and the panes) |
| `%APPDATA%\com.floaty.app\plugins\` | installed third-party plugins |
| `%APPDATA%\com.floaty.app\floaty.log` | backend log, including forwarded frontend errors |

Frontend errors and rejections are forwarded to that log through `floaty_log`, so
a broken widget says why instead of going quiet.

Exactly one process owns that store. Floaty holds a mutex named after its
identifier, so a second launch hands its arguments to the instance already running
— which raises its settings window — and exits before it can load the store. The
second launch is a request to see Floaty, not a second desktop.

Start with Windows is the one setting that does not live in those files: it is a
`Run` entry under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, written when
you turn it on and removed when you turn it off (Windows' own Startup tab in Task
Manager shows the same entry). Launch reconciles the two, which is how the entry
survives an update that installs the app somewhere new; and if the write fails, the
switch goes back to where it was rather than promising a start that will not
happen.

![The widget tray: notes, clock, Live2D, visualizer, system monitor](../screenshots/widgets.png)

---

[<- Back to the README](../README.md)
