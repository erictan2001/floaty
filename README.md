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

A **settings window** (tray icon → Settings) adds widgets, scans/floats apps,
lists what's on the desktop, and removes widgets. Everything — positions,
pinned states, note text, resolved icons — persists in `floaty-store.json`
and restores on launch.

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
  main.ts        mounts note / clock / pet / launcher by route
  settings.ts    settings page (add, scan, float, remove)
  style.css      all widget + settings styling
  widgets/
    lib.ts       window-pos helpers (logical px), store access, drag bar
    note.ts      sticky note
    clock.ts     clock + pomodoro
    pet.ts       wandering pet (manual drag, hover-stop, dblclick pause)
    appicon.ts   gravity launcher (manual drag, pin / drop / launch, icons)
    folder.ts    group folder (drag-drop grouping, expandable launch grid)
src-tauri/
  src/lib.rs     store, widget windows, tray, app scan, launch, icon resolve
  capabilities/  Tauri ACL grants (window ops, …)
  tauri.conf.json
  icons/         tray + bundle icons (source: assets/icon.png)
```

## Notes

- All window motion uses logical px (`scaleFactor`-converted), so HiDPI
  restores don't drift. Widget loops never drive the window before the stored
  position loads, and never hold the store lock across file IO.
- Pet and launchers use manual cursor-following drags instead of
  OS-level `startDragging`, which swallowed click/double-click sequences and
  raced the motion loops.
- Window create/close runs off the command thread — blocking it on the
  main-thread dispatch froze the settings UI.
- App icons sit at desktop level (`always_on_top` off); notes/clock/pet stay
  above other windows.
- Frontend errors/rejections are forwarded to the backend log via
  `floaty_log`. Log + store live in `%APPDATA%\com.floaty.app\`
  (`floaty.log`, `floaty-store.json`).
- Run one instance at a time — there is no single-instance guard yet, and two
  backends sharing one store step on each other.
