# What "mature" means for Floaty

The answer to "how do I make this project more mature" is not a list of artefacts. It is a
target and the order that follows from it. The target is recorded in
[ADR 0001](adr/0001-strangers-depend-on-this.md): **strangers depend on this**, and
"depend" means trustworthiness, not support.

## The bar

A stranger installs a release build on a machine that has never run Floaty, adopts a folder
with real files in it, merges two items, undoes it, and takes an update - and nothing they
own is lost or moved without their hand. Verified on a clean Windows VM first; a second
person using it for a week is the step after that.

## The programme, in order

1. **The two known trust bugs.** `verify/drag.mjs` merges real files when it runs, and
   `test_windows_startup_configuration` writes into the real user profile. A probe that
   moves the user's files and a test that rewrites their settings are exactly the failures a
   stranger must never be handed.
2. **Dependabot, security patches only.** Everything else stays pinned.
3. **CHANGELOG, the plugin-API deprecation window, and version files bumped at tag time**
   ([ADR 0003](adr/0003-two-version-numbers.md)).
4. **A nightly CI tier** that runs the six probes needing a running app, the whole `cargo test`
   suite and a real NSIS installer build on a clean runner. Pull requests keep the fast gates only, so a one-line change stays quick.
5. **Vitest over the pure logic in `src/widgets/lib.ts`** - placement, geometry, the
   physical-to-virtual mapping, presence. That is where the coordinate bugs have actually
   lived, and it needs no app, no window and no Chrome.
6. **The updater work** ([ADR 0002](adr/0002-withdraw-bad-releases.md)): smoke-test the built
   installer before upload, refuse a manifest whose version disagrees with its tag, leave the
   old install running when an update fails.
7. **The first-run guard** ([ADR 0004](adr/0004-first-run-blocks.md)).
8. **The docs updated to state all of it.**

That is a **v0.3.0**. The guards are the release, not a patch on top of one.

## Where it stands

Done and verified: **1** (both trust bugs - the startup test saves and restores the
profile values, and `verify/drag.mjs` creates its own throwaway items and cleans up),
**2** (dependabot, security patches only), **3** (the CHANGELOG, the one-release
deprecation window, and `sync-main-version` bumping the three version files when a release
succeeds), **4** (the nightly tier - the six probes that need a running app, the whole
`cargo test` suite and a real installer build, with the six recorded as skipped rather than
counted green on a runner that has no desktop), **5** (vitest over the pure logic in
`lib.ts`, 44 tests) and **6** (the updater guards: draft, check, publish). **8** is done, including
`verify/README.md`, which now says how a probe pre-seeds the first-run answer.

Still open: **7** (the first-run guard). Nothing is claimed done here until it has been run
and checked.

One gap stays open by decision: the cross-screen drag leg in `verify/drag.mjs` cannot run on
a machine that reports one overlay, so the boundary drag itself is unproven. The other —
nothing before publication installs the installer and opens the app — belongs in the
clean-machine check above, and is carried as debt in [the backlog](BACKLOG.md#process).

## Deliberately not being done

- **No telemetry, no crash reporting, no support SLA, no cloud dependency** — the constraint
  the target was chosen under ([ADR 0001](adr/0001-strangers-depend-on-this.md)). Anything
  that needs an endpoint to stay alive is out, which is also why the updater has no kill
  switch, only withdrawal.
- **Nothing for contributors yet**: no CONTRIBUTING, no issue templates, until the guards
  and the release path are done. The [backlog](BACKLOG.md) is the list, not a tracker.
- **No major-version chasing.** A pixi 6 to 8 migration is a week this project does not
  have; a security patch is not negotiable.
