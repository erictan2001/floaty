# Changelog

Notable changes to Floaty, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the app version follows
[semantic versioning](https://semver.org/).

The plugin API version is a different number with a policy of its own — see
[Plugins](docs/PLUGINS.md#the-plugin-api-version).

## 0.2.1 - 2026-09-24

### Added

- TypeScript unit tests (vitest, `npm test`) over the pure logic in `src/widgets/lib.ts`:
  placement, geometry, the physical-to-virtual mapping, presence.
- A nightly CI tier: the whole `cargo test` suite, the six probes that drive the running
  app, and a real NSIS installer build, on a clean runner.
- Dependabot, for security patches only. Everything else stays pinned.
- A first-run guard: floaty shows the folder it is about to adopt, and what is in it, and
  waits for confirmation. Later root changes warn and proceed.

### Changed

- The app version is written into `package.json`, `src-tauri/tauri.conf.json` and
  `src-tauri/Cargo.toml` at tag time, so `main` always states the last released version.
- The plugin API version is separate from the app version, and changes only when the
  surface a plugin sees changes.
- A plugin API break needs a bump, a line here, and a deprecation window of one release,
  during which a manifest carrying the old number still loads and warns.
- A bad release is withdrawn rather than kill-switched: delete the release and its
  `latest.json` and publish a fixed one. A copy that has not updated fails visibly and
  keeps running the build it has. Installs old enough to predate the stricter updater may
  need one manual reinstall.

### Fixed

- `verify/drag.mjs` no longer merges real files when it runs. It creates its own
  throwaway items and cleans up after itself.
- `test_windows_startup_configuration` no longer writes `StartupDelayInMSec` and
  `WaitForIdleState` into the user's profile.

## 0.2.0 - 2026-09-23

The release that made the desktop every screen, and the first with an updater.

### Added

- **One overlay per screen.** Floaties live on any monitor and can be dragged between
  monitors mid-drag, each screen rendering at its own scale factor. A fullscreen app takes
  only its own screen away, and a floatie is brought back onto a screen that still exists.
- **A launcher palette.** `Ctrl+Alt+Space` opens a search box on the screen the pointer is
  on.
- **Undo and redo**, including the disk moves an undo reverses.
- **Plugins from a `.zip`**, with inspect and uninstall, and an approval step: an
  installed plugin is not loaded until it has been approved, and replacing its files asks
  again.
- **Plugin API version 2** — `notify`, `every`, `after` and `on` for timers and floaty's
  own events. A plugin written against version 1 keeps working.
- **A Diagnostics tab**: the log tail, the monitor inventory, the window positions and the
  heartbeats.
- **Automatic updates**, checked only when you ask.
- **Icons as files**, with a tidy pass, so the desktop mirrors what is in the root folder.
  Files dropped in from outside are picked up too.

### Changed

- Starting floaty with Windows is faster: launch is prioritised and the startup delay is
  gone.
- The README was split into beginner docs — [Concepts](docs/CONCEPTS.md),
  [Widgets](docs/WIDGETS.md), [Working on Floaty](docs/DEVELOPING.md).
- `lib.rs` was split into one module per section (9710 lines down to 1324), and the tree
  was made clippy-clean.

### Removed

- **The pet widget has been removed** — a breaking change, and the reason this release is
  0.2.0 rather than 0.1.2. `pet_speed` went with it: it is no longer part of the
  documented `api.settings()` surface, so a plugin that read it stops finding it. That
  break landed with no plugin API bump and no record anywhere; the policy that now
  requires a bump, a line here and a one-release deprecation window is under
  [0.2.1](#021---2026-09-24).

### Fixed

- A widget's whole body stays on the desktop, not just its corner.
- A drag gesture that ends nowhere brings the widget back, and the release places the
  widget before it ends the gesture.
- The palette asks again when it is summoned, so it opens with everything.
- The page listens for the machine's quiet from boot, and a fullscreen app takes only its
  own screen away.
- The icon store stays small: nothing the resolver hands out is inline.

## 0.1.1 - 2026-09-15

### Added

- The MIT license, and a note in the docs that floaty is built with AI assistance.
- An examples README.

### Changed

- The motion settings have one slider per idea.

### Fixed

- The wave animation actually ripples.

## 0.1.0 - 2026-09-15

The first release.

### Added

- **The floating desktop.** Point floaty at a folder and what is in it falls, bounces and
  piles up on the wallpaper: shortcuts launch apps, documents are file icons, subfolders
  are folders. The folder is watched, so a file added, renamed or deleted in Explorer
  appears, moves or vanishes on the desktop with no sync button.
- **Real files.** Dropping one floatie onto another makes a real folder and moves the
  files into it on disk; removal goes to the Recycle Bin; app icons become shortcuts.
- **A merge preview** — dropping an icon squarely on another shows the folder it is about
  to become.
- **Widgets**: sticky notes, a pomodoro clock, a Live2D companion, an audio visualizer and
  a system monitor.
- **A tabbed settings window**, one home per setting, wearing the same panel theme as the
  widgets.
- **A plugin architecture**: built-in and user-installable plugins, enable toggles,
  per-plugin settings, and one source of truth per plugin fact.
- **The trail plugin**, with named save slots, thumbnails, quick save and quick load —
  draw a path and the icons line up along it, and an arrangement survives a restart.
- **A pin-on-top layer** for widgets, and a desktop layer for icons.
- **A switch that starts floaty with Windows.**
- **A release pipeline**: a tag builds and publishes both architectures, building the
  frontend before any Rust step.

### Changed

- GPU use was cut with phased animations; Live2D responsiveness and the settings UI were
  improved alongside it.
- The desktop follows the pointed folder instead of waiting for a sync button.

### Fixed

- A settings edit reaches the widgets it changes, and the log says so.
- The overlay is recovered after sleep, and the store no longer stalls behind it.
- A falling icon lands on the file icons below it instead of through them.
- The settings window is an ordinary window again — minimize, drag, and no duplicate close
  button.
- A slot stretched to catch the mouse is not the widget's place.
