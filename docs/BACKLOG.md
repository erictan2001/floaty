# Backlog

Known work, unfixed, written down the moment it is found. A line here is not a promise to
do it; it is a promise not to forget it. When something is fixed, delete the line.

## Known bugs

- `verify/stranded.mjs` leaves one step on the shared undo stack per run: it pushes two (the
  placed drag and the pointer drag) and unwinds one. Its own output prints the depth before
  and after, so the leak is visible, but a probe should leave the stack no deeper than it
  found it.

- The Rust tests write their fixtures into the system temp folder and never remove them:
  thousands of directories accumulated from repeated `cargo test` runs (`floaty-plugin-*`,
  `floaty-icons-*`, `floaty-shellops-restore-*`). Individually tiny, collectively a mess a
  person sees when they look at `%TEMP%`. A `Drop` guard would fix it; the verify side has
  the same shape and now sweeps its own stale folders (`launchChrome`).

- A fresh widget is born where the backend's cascade puts it (`140 + (n*47 % 480)`,
  `140 + (n*31 % 320)`), which knows nothing about the desktop, and the overlay's nudge only
  runs for a record with no place of its own — so a new note or clock can land on top of an
  icon and stay there. `placement.freeSpot` would clear it; the gate, not the policy, is what
  stops it.
- A kind's size limits live in two places and already disagree in one: the table declares them
  inside `custom_size`'s closure (`size_from_wh(base, data, min, max)`), so nothing outside
  `plugins.rs` can read them, and each widget retypes its own minimum for the resize handle —
  `visualizer` is 140×70 in the table and 160×80 in the widget, `note`/`clock`/`sysmon` and
  the countdown example agree with their manifests only by hand, and the trail example's panel
  is 146 high while its manifest's `minSize` is 182. The backend clamps the saved size to the
  table's numbers, so the retyped one is a grip that stops somewhere the record does not:
  declaring the limits as data (and carrying them on the wire, where `resizable` already goes —
  it is declared in the payload and in `pluginManifest.ts` and read by neither, and it is
  missing from `check-plugins.mjs`'s `WIRE_FIELDS`, so a rename there is silent) would make
  them one owner.
- The path key is honoured in `widget_path`, `record_path` and the launcher, and hardcoded
  everywhere else: `app_discovery_launch.rs` reads `"target"` at :692 and :745,
  `putting_dragged_items_on_disk.rs` reads `"path"`/`"target"` at :335, :502, :698, :784, :895
  and :899 (and writes them at :612, :757, :765, :807, :985, :1008), and
  `global_floating_settings.rs` tries `["path", "target"]` at :549 and :635. An installed plugin
  with `pathKey: "where"` therefore works in three places and is invisible in the rest, and the
  readers that guess can hand a non-path value to a shell verb. `plugins::path_key(kind)` is the
  accessor; the writers need the same rule, which is why this is a pass of its own rather than a
  rename.
- Path placement decides whether a name is free by looking, and then moves; Windows' `rename`
  writes over whatever is there, so two placements racing for one name is the case
  `placement.rs` does not close (a reservation would). Nothing has hit it yet.
- The first overlay load after a dev-tree restart takes ~45s on this machine: a cold Vite
  serves `src/bootstrap.ts` in 45s the first time and 30ms after that, so a probe that samples
  the DOM once reports "0 mounted, 44 belong here" on an app that is perfectly healthy — and
  `verify:app reload` fixes it, because the second load is warm. Probes must wait for the mount
  (the runtime's `until`, ~60s) instead of sampling once. That is also the shape of the older
  "`app-ready` times out even when the app is up" report, which has not reproduced since the
  liveness check moved into `attach()`.
- No `.gitattributes`: the tree is CRLF and mixed endings churn diffs.

## Process

- `PLUGIN_API_VERSION` is still 2. The pet removal broke the documented `api.settings()`
  surface in the same week the policy requiring a bump for exactly that was written, so the
  next release should carry API version 3 and a CHANGELOG line. Bumping it alone would be
  wrong: a plugin declaring a number the released app does not speak is refused, so the bump
  has to ride an app release.
- Nothing before publication installs the built installer and opens the app, so nothing yet
  proves an installer installs and the app opens - the clean-machine check in
  [MATURITY](MATURITY.md) is where that belongs. What the guards do cover, and why the other
  two gaps are accepted, is in
  [RELEASING](RELEASING.md#what-is-checked-before-a-release-goes-out).
- `scripts/verify-updater-feed.mjs` downloads the release assets, so a network hiccup fails
  the run closed and leaves the release a draft, needing a re-run. Correct but annoying.
- `sync-main-version` pushes its bump commit with `GITHUB_TOKEN`, so that commit does not
  trigger CI, and branch protection on `main` would reject it outright. The job fails loudly
  and `npm run sync:version -- vX` is the manual repair, but the gap is real. A withdrawn
  release also leaves `main` claiming a version that is no longer live.

## Unfinished features

- Keyboard operation of the desktop (deferred: the first implementation was buggy).
- A systematic audit that undo is correct for every action, not only the tested ones.
- The install-button file dialog cannot be driven headlessly, so that path is untested.
- Theme knob: accent colour and panel opacity are hardcoded.
- Settings export / import / reset.
- Per-core CPU and disk/network rows in the system monitor.
- A third example plugin.
