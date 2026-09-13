# Floaty — widgets living on your desktop

Rust + Tauri v2 desktop app (Vite + TypeScript frontend). Widgets are drawn
inside one full-screen transparent overlay, so the rest of your desktop stays
clickable; a per-widget window path still exists (`index.html#/<kind>/<id>`).

## Widgets

- **Sticky notes** — drag by the top bar, text autosaves.
- **Clock + pomodoro** — live clock, focus/break timer with round tracking.
- **Pet blob** — wanders your screen on its own. Hover to make it sit still,
  double-click to freeze/unfreeze it, drag it anywhere by its body.
- **Gravity app launchers** — real apps, auto-discovered from Start Menu +
  Desktop. Drag one anywhere and it pins there; single-click drops it with
  gravity (bounces, piles onto other icons); double-click launches the app
  in place. Shows the app's real icon when one can be resolved.
- **Group folders** — drag one app icon onto another to group them (or drop
  icons onto a folder). Click a folder to expand it into a launch grid;
  click again to collapse. Remove via the × like any widget.
- **Desktop files & folders** — point Floaty to any root directory. Files and
  subdirectories become interactive floaties. Dragging a file or directory out of
  a folder moves it on disk into the desktop parent directory and spawns it as a new
  desktop floatie. Dragging files/folders onto folder floaties moves them into the folder on disk.
- **Live2D companion** — interactive desktop companion (pixi + Cubism 2/3/4
  runtimes). Follows cursor across desktop, reacts to head/body taps, idles,
  and drags anywhere. Point its plugin card at any model library folder
  and pick per-widget models from the scan (Cubism 2 `.model.json` / Cubism 3+ `.model3.json`).

A **settings window** (tray icon → Settings; the tray itself is just Settings
+ Quit) hosts the plugin manager, scans/floats apps, lists what's on the
desktop, and removes widgets. Everything — positions, pinned states, note
text, resolved icons — persists in `floaty-store.json` and restores on
launch. Disabling a plugin closes its windows outright (records are kept);
nothing disabled is loaded, listed, or creatable until re-enabled.

## Run (dev)

```powershell
npm install
npm run tauri dev
```

Tray icon: add widgets, open settings, quit. Useful env harnesses:

```powershell
$env:FLOATY_OPEN_SETTINGS=1   # auto-open settings on boot
$env:FLOATY_DEMO=1            # float a few sample apps + clock + pet
```

## Build

```powershell
npm run tauri build
```

Unsigned exe/NSIS installer lands in `src-tauri\target\release\bundle\`.
Windows 10 needs the WebView2 runtime (Win11 ships it).

## Project structure

```text
index.html / settings.html  entry pages (widget host / settings UI)
src/
  bootstrap.ts   error forwarding + widget router (hash: #/note/:id …)
  main.ts        mounts the routed plugin
  settings.ts    settings page: plugin manager, app scan, desktop list
  style.css      all widget + settings styling
  widgets/
    plugin.ts       plugin registry & FloatyPlugin interface
    live2dPlugin.ts lazy-loading wrapper for Live2D
    lib.ts          window-pos helpers, store access, drag bar, settings cache
    note.ts         sticky note plugin
    clock.ts        clock + pomodoro plugin
    pet.ts          wandering pet plugin
    appicon.ts      gravity launcher plugin
    folder.ts       group folder plugin
    live2d.ts       Live2D companion (lazy pixi chunk, Cubism 2/3/4 model)
src-tauri/
  src/lib.rs        store, widget windows, tray, scan, launch, icons
  src/plugins.rs    backend plugin registry, sizing, resizability, default data
  capabilities/     Tauri ACL grants (window ops, …)
  tauri.conf.json
  icons/            tray + bundle icons (source: assets/icon.png)
```

## Plugins

Every floatie is a modular plugin. A plugin is described twice, on purpose: the
backend manifest (`src-tauri/src/plugins.rs`) owns names, sizes, the quick-add
label and whether the kind stands for a file on disk, and the frontend module
(`src/widgets/*.ts`, listed in `src/widgets/plugin.ts`) owns behaviour — mounting,
describing, settings rows. The frontend reads the manifest, so those facts exist
once; `npm run build` runs `scripts/check-plugins.mjs`, which fails when the two
id lists disagree.

**Third-party plugins need no floaty source at all.** Drop a folder with
`plugin.json` + `index.js` into `%APPDATA%\com.floaty.app\plugins`, press
**rescan plugins** in settings, and the widget is available — same manifest, same
API, same settings card as a built-in. Installing, the full `plugin.json`
reference, the module contract and the `api` object are in
**[docs/PLUGINS.md](docs/PLUGINS.md)**; a complete worked example lives in
[`examples/plugins/countdown`](examples/plugins/countdown).

The settings **Plugins** section lists every plugin (built-in and installed) with
enable toggles and per-plugin parameter cards, plus the buttons that open the
plugins folder and rescan it. Disabling a plugin closes its widgets (records are
kept); re-enabling respawns them; creating a widget re-enables its plugin.
Global values live in `floaty-settings.json` with per-widget data in
`floaty-store.json`.

## Notes

- All window motion uses logical px (`scaleFactor`-converted), so HiDPI
  restores don't drift. Widget loops never drive the window before the stored
  position loads, and never hold the store lock across file IO.
- Pet and launchers use manual cursor-following drags instead of
  OS-level `startDragging`, which swallowed click/double-click sequences and
  raced the motion loops.
- Window create/close runs off the command thread — blocking it on the
  main-thread dispatch froze the settings UI.
- Everything sits at desktop level under real apps by default; right-click
  any widget for a pin-on-top toggle (persisted per widget).
- Frontend errors/rejections are forwarded to the backend log via
  `floaty_log`. Log + store live in `%APPDATA%\com.floaty.app\`
  (`floaty.log`, `floaty-store.json`).
- Run one instance at a time — there is no single-instance guard yet, and two
  backends sharing one store step on each other.
