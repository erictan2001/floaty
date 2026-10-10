# Good first issues (draft)

**Status: draft.** Nothing here is created on GitHub yet. Each item is taken from
[BACKLOG.md](BACKLOG.md) and each file:line was read in the code while writing this.
Line numbers drift; re-check before filing.

Labels for all of them: `good first issue`, `help wanted`, plus one area label.

---

## 1. Use the theme accent variable in the pin-row checkbox

- **Title:** `Theme: pin-row checkbox uses hardcoded accent #6c5ce7`
- **Labels:** `good first issue`, `help wanted`, `area: theme`
- **Body:**
  - BACKLOG.md, "Unfinished features": *Theme knob: accent colour and panel opacity are hardcoded.*
  - `src/styles/floatie.css:698` sets `accent-color: #6c5ce7;` while `src/styles/base.css:4` already defines `--accent: #6c5ce7;`.
  - Replace the literal with `var(--accent)` so one variable controls the accent.
- **Acceptance:** `grep -n "6c5ce7" src/styles/floatie.css` returns nothing; `npm run build` and `npm test` pass; the pin checkbox still renders purple.

## 2. Make panel opacity a CSS variable

- **Title:** `Theme: panel background alpha is hardcoded in --panel-bg`
- **Labels:** `good first issue`, `help wanted`, `area: theme`
- **Body:**
  - BACKLOG.md, "Unfinished features": *Theme knob: … panel opacity are hardcoded.*
  - `src/styles/base.css:13` writes the alphas `0.84` and `0.74` directly inside `--panel-bg`.
  - Add a `--panel-opacity` (or two alphas) in `:root` and build `--panel-bg` from it. Keep the default values identical.
- **Acceptance:** `npm run verify:panes` and `npm run verify:palette` pass against `npm run dev`; `--panel-bg` resolves to the same gradient as before (the palette and the desktop panels both read it).

## 3. Delete the unused `manifestFor` export

- **Title:** `Remove unused manifestFor from pluginManifest.ts`
- **Labels:** `good first issue`, `help wanted`, `area: frontend`
- **Body:**
  - BACKLOG.md, "Known bugs": *`pluginManifest::manifestFor` (`pluginManifest.ts:94`) is exported and read by nobody.*
  - Verified at `src/widgets/pluginManifest.ts:99-101`; a repo-wide grep finds no caller (the line number in the backlog is stale).
  - Delete the three-line function. Its neighbours walk `entries` directly, so nothing else changes.
- **Acceptance:** `grep -rn manifestFor src verify scripts examples` returns nothing; `npx tsc --noEmit` and `npm test` pass.

## 4. Rename `floaty_dropped_inner` to say what it does

- **Title:** `Rename floaty_dropped_inner: it is the group/merge path, not a drop`
- **Labels:** `good first issue`, `help wanted`, `area: backend`
- **Body:**
  - BACKLOG.md, "Known bugs": *Names left behind by the refactor: `floaty_dropped_inner` (`disk_drag.rs:381`) is the whole group/merge path now, not a drop.*
  - Definition at `src-tauri/src/disk_drag.rs:407`; callers at `:395`; the name is also quoted in a comment at `src-tauri/src/diagnostics_commands.rs:758`.
  - Pick a name that says "group/merge" (for example `group_on_drop`), rename the fn and the three references, and fix the doc comment at `:381`.
- **Acceptance:** `grep -rn floaty_dropped_inner src-tauri` returns nothing; `cargo test --lib` and `cargo clippy -- -D warnings` pass (run from `src-tauri`, with the app closed).

## 5. Fix the example-plugin count in examples/plugins/README.md

- **Title:** `Add a third example plugin (examples/plugins says three; there are two)`
- **Labels:** `good first issue`, `help wanted`, `area: plugins`
- **Body:**
  - BACKLOG.md, "Unfinished features": *A third example plugin.*
  - `examples/plugins/README.md:3` says "Three complete plugins", and the table at `:10` lists two. Folders present: `countdown/`, `trail/`.
  - Add a small self-contained plugin (`plugin.json` + `index.js`, no dependencies, only the documented `api`), then add its row to the table. Read `docs/PLUGINS.md` first.
- **Acceptance:** `npm run check:examples` passes; `cargo test --lib the_examples_validate_like_any_other_plugin` passes (`plugins.rs:1226`).

---

## Judged too big for a newcomer

- **Settings export / import / reset:** new IPC commands, persistence and UI across several panes.
- **A systematic audit that undo is correct:** a review of every action, not a single change.
- **Keyboard operation of the desktop:** deferred for a reason; the first attempt was buggy.
- **Install-button file dialog:** cannot be driven headlessly, so the acceptance check needs a person at a desktop.
- **Per-core CPU and disk/network rows:** new sampling in `sysmon.rs` plus UI rows.
- **`store::is_dead` / tombstone rules:** correctness-sensitive store invariants.
- **Path key in writers (`disk_drag.rs`, `settings_sync.rs`):** a pass of its own, per the backlog.
- **`verify/panel-drag.mjs` sampling:** needs a desktop run to validate.
- **Release and signing items:** need repository secrets or a clean machine.

## Backlog entries checked and not included

- The visualizer "160×80 in the widget" minimum (BACKLOG.md, Known bugs): `src/widgets/visualizer.ts` has no hardcoded min; `resizeHandle` reads `minSizeFor` (`floatie.ts:533`). Appears already fixed; not verified as a bug.
- The trail "panel is 146 high" claim: `examples/plugins/trail/plugin.json:9-11` is 268×182 with `minSize` 268×182. Appears already fixed.
- `resizable` missing from `check-plugins.mjs` `WIRE_FIELDS`: it is present at `scripts/check-plugins.mjs:74`. Appears already fixed.
