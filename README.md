# Floaty — widgets living on your desktop

Rust + Tauri v2 desktop app (Vite + TypeScript frontend). Every widget is its
own small borderless window, so the rest of your desktop stays clickable.

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
- **Live2D companion** — animated Hijiki model (pixi + Cubism 2 runtime
  vendored). Idles on its own, reacts to taps, drags anywhere by the body.
  Point its plugin card at any model folder and pick per-widget models
  from the scan (Cubism 2 `.model.json` / Cubism 3+ `.model3.json`).

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
    plugin.ts    plugin registry (kind, name, mount, list labels)
    lib.ts       window-pos helpers, store access, drag bar, settings cache
    note.ts      sticky note plugin
    clock.ts     clock + pomodoro plugin
    pet.ts       wandering pet plugin
    appicon.ts   gravity launcher plugin
    folder.ts    group folder plugin
    live2d.ts    Live2D companion (lazy pixi chunk, Cubism 2 model)
src-tauri/
  src/lib.rs     store, widget windows, tray, plugins, scan, launch, icons
  capabilities/  Tauri ACL grants (window ops, …)
  tauri.conf.json
  icons/         tray + bundle icons (source: assets/icon.png)
```

## Plugins

Each floatie is a plugin registered in `src/widgets/plugin.ts` (kind, name,
mount function, list labels) with matching backend support (`default_data`,
`widget_size`, create allowlist in `src-tauri/src/lib.rs`). The settings
**Plugins** section lists them with enable toggles and per-plugin parameters
(pet speed lives on the pet card; gravity, bounce, float and click behaviour
on the launcher card). Disabling a plugin closes its windows (records are
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
