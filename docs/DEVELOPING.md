# Working on Floaty

How to build it, run it while you change it, and check your work before it
goes anywhere.

## Requirements

- Windows 10 or 11 (Win32 shell, Recycle Bin and WASAPI loopback are used
  directly). Windows 10 needs the WebView2 runtime; Windows 11 ships it.
- Rust (stable) and Node 18+ for building from source.

## Building

### Development

```powershell
npm install
npm run tauri dev
```

`tauri dev` starts Vite on `http://localhost:1420` and a debug build that loads its
pages from it, so the frontend reloads as you edit while the Rust side is
recompiled and the app restarted when that changes. Two environment harnesses help
when reproducing something:

```powershell
$env:FLOATY_OPEN_SETTINGS=1   # open the settings window on boot
$env:FLOATY_DEMO=1            # float a few sample apps and a clock
```

Two things worth knowing before they cost you an afternoon:

- A running app locks its own executable, so `cargo build` (and the link step of
  `cargo test`) fails with `Access is denied` until you quit Floaty. The dev tree
  rebuilds by itself, so close the app when you want to build or test by hand.
- A debug build expects that dev server: launch it with nothing listening on
  `:1420` and the windows come up empty.

## Checks

```powershell
npm run build              # plugin id check + tsc --noEmit + vite build
npm run check:plugins      # only the backend/frontend plugin id agreement
npm test                   # vitest: the pure logic in src/widgets/lib.ts
cargo test                 # 80 backend tests, run from src-tauri
cargo check
npm run verify:panes       # render every settings pane headlessly (needs npm run dev)
npm run verify:app         # check the running app over its debug port
```

`verify/` holds the probes for the two halves `cargo test` cannot reach: the pages,
and the running app. They need no dependencies and they talk to the real thing — the
pane probe drives the real pane code with the IPC stubbed, the app probe drives the
real store through the real IPC. [verify/README.md](../verify/README.md) says what each
one catches and the four rules for writing another.

`npm test` runs the TypeScript unit tests (vitest) over the pure logic in
`src/widgets/lib.ts` — placement, geometry, the physical-to-virtual mapping and
presence. It needs no app, no window and no Chrome, which makes it the fastest of the
three and the one to reach for first: that is where the coordinate bugs have lived.

The backend tests cover the watcher, the shortcut and grouping rules, the plugin
manifests, the icon store (including a real recycle-and-restore round trip), undo
steps and the disk moves they reverse, the desktop-layer window policy and how an
older settings file loads.
`scripts/check-plugins.mjs` fails the build when the plugin listed in the backend
registry and the one registered in the frontend disagree, which is the one mistake
that is easy to make and invisible at runtime. (Quit Floaty first: the running app
locks the executable the test link step needs.)

## Continuous integration

Two tiers.

**Pull requests** (`.github/workflows/ci.yml`) run the fast gates: `check:plugins`,
`tsc --noEmit`, `npm run build`, `cargo test --lib`, and the two page probes that need
nothing but a dev server — `verify:panes` and `verify:palette`. `verify:app` is
deliberately absent: it talks to a *running* floaty over its debug port, and a
pull-request runner has no desktop to put one on.

**Nightly** (`.github/workflows/nightly.yml`) is the slow half: the whole `cargo test`
suite rather than `--lib` alone, the six probes that drive the *running* app —
`verify:app`, `verify:drag`, `verify:presence`, `verify:redo`, `verify:stranded` and
`verify:panel-drag` — and a real NSIS installer build, which is measured and thrown away.
A hosted runner has no desktop to put the app on, so those six report that they could not
run and are recorded as skipped rather than counted as green; dispatch the workflow with
`start_app: true` from a runner that has a desktop to run them for real. The two page
probes are not repeated here — they already run on every pull request.

![The Diagnostics pane: log tail, monitors, window positions, heartbeats](../screenshots/diagnostics-pane.png)

## Project layout

```text
index.html / settings.html   widget host page / settings UI
src/
  bootstrap.ts   error forwarding + hash router (#/overlay, #/note/:id, …)
  main.ts        mounts the routed plugin, heartbeat for ordinary pages
  overlay.ts     the desktop and top overlays: slot layout, mounting, hit rects
  style.css      all widget and settings styling (one --panel-* theme recipe)
  settings.ts    settings shell: tabs, save plumbing
  settings/
    api.ts       safe invoke + error banner
    dom.ts       group / row / card builders shared by every pane
    params.ts    the setting table: key, label, range, unit
    pane.ts      Pane interface
    store.ts     settings cache, debounced saves, widget list reload
    panes/       desktop, apps, files, motion, plugins, general, diagnostics
  widgets/
    plugin.ts        frontend plugin registry + FloatyPlugin interface
    pluginManifest.ts reads the backend manifest
    lib.ts           store access, settings fan-out, drag, animation, sizing
    physics.ts       stepBody / supportGone: one integrator for every faller
    appicon.ts       app + file launchers (one implementation, two kinds)
    folder.ts        group folders, grid, drag in/out
    note.ts clock.ts live2d.ts live2dPlugin.ts live2dBounds.ts
    sysmon.ts visualizer.ts
src-tauri/src/
  lib.rs        store, overlays and windows, watcher wiring, tray, apps, icons
  plugins.rs    backend plugin registry, manifests, sizing, installed plugins
  plugin_install.rs  install, inspect and remove a plugin archive or folder
  palette.rs    the launcher: its window, its hotkey, and how a query is ranked
  screens.rs    the monitors in both spaces (physical and logical) and the mapping
  diagnostics.rs the log (rotation, tail, sizes) and the report the pane renders
  icons.rs      stored icons: files named by content, and the urls that point at them
  undo.rs       what the last few changes were, so one press can put them back — and
                what they left behind, so the next press can replay them
  fs_watch.rs   the root-folder watcher
  audio.rs      WASAPI loopback capture for the visualizer
  sysmon.rs     CPU / GPU / RAM sampling
  shell_ops.rs  shell-level file operations (shortcuts, icons, recycle)
  main.rs       entry point
docs/PLUGINS.md                 plugin authoring reference
examples/plugins/               worked example plugins, and their README
scripts/check-plugins.mjs       backend/frontend plugin id agreement
scripts/set-version.mjs         write the version into every file that carries one
verify/                        headless probes: the settings panes, the running app
.github/workflows/release.yml   tagged build → checked release + signed updater feed
```

## Notes for anyone working on it

- Positions and sizes are logical pixels everywhere (`scaleFactor`-converted), so
  a HiDPI restore does not drift.
- Panes read from the settings store and load nothing themselves; saves patch the
  store and debounce. That keeps a re-render cheap and stops an unmounted control
  from silently resetting a setting.
- Widgets follow settings through `onSettings(cb)` in `lib.ts`. Never register a
  second `listen("floaty-settings-changed")` in a widget: the cache updater and
  the widget then race, and one of them reads the values from before the change.
- Manual cursor-following drags are used instead of OS-level `startDragging`,
  which swallowed click and double-click sequences and raced the motion loops.
- Window create and close run off the command thread — blocking on main-thread
  dispatch froze the settings UI.
- `cargo check` reports two unused constants, `RDW_ERASE` and `RDW_UPDATENOW`:
  they are the unhired half of the `RedrawWindow` flag set declared next to the
  two that the repaint helper uses. The warning is expected.

## Known limits

- **Windows only.** The shell verbs, the Recycle Bin, WASAPI loopback and the
  desktop-layer window policy are all Win32, so there is no cross-platform path.
- **Release builds are unsigned** unless you sign them, so SmartScreen warns on
  first run and the installer's publisher is unknown. See [Signing](RELEASING.md#signing-it). The
  *updater* archives are signed with this project's own key, which is a different thing
  for a different check — see [Releasing an update](RELEASING.md#releasing-an-update).
- **Releases are built for ARM64 and x64, and no update is looked for unless you ask.**
  The feed carries a `windows-aarch64` entry and a `windows-x86_64` one, because the
  updater keys its manifest by the triple of the machine asking
  ([Releasing an update](RELEASING.md#releasing-an-update)); General → **Updates** → *check for
  updates* is the only request anything makes.
- **The watcher reports names, not contents.** Deliberate: a desktop root is
  usually cloud-synced, where content notifications fire on every write while
  nothing on the desktop depends on a file's contents. Editing a file in place is
  not a desktop change, so nothing has to happen for it.
- **A floatie is drawn by one window, so it is clipped at the screen edge.** An icon
  straddling the boundary between two monitors shows only its half that is on the
  screen it belongs to. Everything is 92px wide, so this only shows for a panel wider
  than the screen it is on.
- **The desktop has a band that belongs to no screen** when the monitors are at
  different scales: Windows reports the second screen's origin in physical px, so in
  the coordinates records are stored in, the primary ends at 1440 and the second
  screen starts at 1920. Nothing lives there — a floatie dropped in it is brought back
  to the nearest screen — and the drag maps per screen, so the gap is never visible.
- **The fullscreen check is a poll, every two seconds.** So the desktop can stay up
  for up to two seconds after a fullscreen app takes over, and comes back just as late
  once it closes.

## Built with AI

Floaty was written with AI coding agents in the loop. The implementation, the
refactors, most of this documentation, and a good deal of the investigation behind
the awkward parts — the Win32 window policy, the visibility repair after a display
wake, the trail arrangement — were produced by agents working from directions, bug
reports and measurements. The decisions were a person's: what Floaty is, what it
does on a real desktop, what was worth measuring and what was allowed to ship.
Agents propose; they do not decide.

Two things follow, and they are worth saying plainly rather than leaving implied:

- **Read it before you trust it.** Generated code can be confidently wrong, and the
  parts that touch Win32, the shell and the Recycle Bin deserve a second pair of
  eyes before you run them on a machine you care about.
- **The license is a license, not a warranty.** Nothing here is certified, and the
  [known limits](#known-limits) above are real limits rather than modesty.

Where this README states a measurement, it came from a test or a probe run against
the running app rather than from an agent's summary. Where it states a rule — the
one `onSettings` registry, the integer coordinates, a saved position being the
truth — the rule is there because breaking it caused a bug, and the bug is in the
commit history.

## License

MIT — see [LICENSE](../LICENSE): use, copy, modify, merge, publish, distribute,
sublicense or sell it, keeping the copyright notice. It comes with no warranty.

---

[<- Back to the README](../README.md)
