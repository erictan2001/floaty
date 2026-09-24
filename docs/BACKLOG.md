# Backlog

Known work, unfixed, written down the moment it is found. A line here is not a promise to
do it; it is a promise not to forget it. When something is fixed, delete the line.

## Known bugs

- `verify/app-ready.mjs` times out even when the app is up and healthy, so every probe that
  waits on it can report a false negative.
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
