# verify

Probes for the two halves of floaty that `cargo test` cannot reach: the pages, and
the running app. Every one of them was previously written by hand in a temp folder,
used once, and thrown away — which meant the next person re-derived it, and a
regression could only be found by looking at the screen.

Runs are independent: each one picks its own free debug port and its own Chrome
profile, so a probe can run while another probe (or the app's own debug port on 9222)
is up.

There are no dependencies here on purpose. Node 21+ has a global `WebSocket`, and
CDP needs a target list, one method call at a time, and a key event — puppeteer and
playwright would each be 100MB of a second browser to drive the one we are actually
testing.

| Run | What it does | Needs |
| --- | --- | --- |
| `npm run verify:panes` | renders all six settings panes from the dev server with the Tauri IPC stubbed, one Chrome per run, and fails on an exception, a console error, a pane that drew nothing, or a row wider than the window | `npm run dev` |
| `npm run verify:app` | checks the *running* app over its debug port: mounted slots vs records, icons that failed to load, whether the page believes it is visible, console errors | see below |
| `npm run verify:arrangement` | sends the display-change message Windows sends, to every window, and reads the log back: the windows and pages must refit at once, and the *desktop* must not be rearranged for the reported shape until that shape has stopped changing | the running app |
| `npm run verify:image-thumbnail` | adds four generated images as real floaties through the same path a dropped photo takes, then reads back the icon each record holds: its own picture at its own proportions within 256px, distinct bytes per file — and deletes the records on the way out | the running app |
| `npm run verify:palette` | renders the launcher palette with its search stubbed, types into it, and asserts the keys do what they say | `npm run dev` |
| `npm run verify:drag` | drags a real floatie from one screen to the other with real input events, then checks it arrived, landed where the pointer held it, is drawn by exactly one window, and that no file was merged on the way | see below, two screens |
| `npm run verify:trail-page` | loads the real trail module into the real overlay with the IPC stubbed, then opens the save page and the drawing surface, escapes each, and checks the panel came back to its own size and place — and that no save along the way recorded the screen's size as the panel's | nothing; it starts its own vite |

## A page probe, and what it is for

`verify:trail-page` is the shape of the others that do not need the real desktop: the
overlay page is loaded from the dev server with the IPC stubbed, in a headless Chrome of
its own, so nothing is written to the user's store, data folder or log. **The stub is the
world** — it holds exactly one record, the panel under test — which is the whole point
for a probe about one widget's size: a desktop full of icons only makes the numbers harder
to read.

It exists because of a bug a unit test cannot see. `api.setPos` could say *this stretch is
scaffolding, do not record it*; `api.setSize` could not, and since the session refactor a
`setSize` on your own widget writes `data.w/h` — so a plugin covering the desktop
overwrote its panel's size in the record, and the restore then read the screen's size back
out of it. The report was "load, then esc, and it will not resize back".

Three things make it worth having:

- **It drives the real module and the real overlay.** Not a reimplementation: the page
  imports `examples/plugins/trail/index.js` the way the app does, and `Floatie.size` is
  the code under test. A stubbed `api.setSize` would pass with the bug in place — which is
  exactly how this was live for so long.
- **Both gestures, and the save.** The save page and the drawing surface stretch the slot
  two different ways, and fixing one is the easy mistake. Neither one *saves*, so the
  drawn size recovers on its own and a probe that only looks at the slot reports a
  self-healing glitch; the record is what stays wrong, and it becomes permanent the moment
  anything saves it while the slot is still stretched. The probe forgets a saved slot
  inside the open page to make that save happen — pre-fix that is where **1280x800** lands
  in the store.
- **A failing run reads the same as a passing one.** `settle()` hands back what the page
  actually looks like when a predicate never comes true, so the report says
  `1280x800 at 40,40` rather than "nothing" — the number is the diagnosis.

Its dev server is started with `--force` on a free port: Vite caches modules outside
`src/`, and the trail's module is served from the project root, so an edit that keeps
coming back as the previous version reads exactly like a fix that did nothing.

## Checking the running app

The overlay is a real WebView2 window, so the only way in is its debug port. Start
the app with it open:

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
npm run tauri dev
```

`--nth N` picks between windows that share a page: with one overlay **per screen**,
every overlay is `index.html#/overlay`, so `health` walks all of them and checks each
window against the records that fall on its own screen — plus the sum, because the
failure that matters is a floatie no window drew (or two that drew it).

```powershell
npm run verify:app -- --nth 1 js "await window.__TAURI_INTERNALS__.invoke('floaty_overlay_area')"
npm run verify:app                                   # health
npm run verify:app reload                            # reload the pages (stale HMR)
npm run verify:drag                                  # drag a floatie across screens
npm run verify:app -- js "await window.__TAURI_INTERNALS__.invoke('floaty_undo_state')"
npm run verify:app -- key ctrl+z                    # a real key event
npm run verify:app -- shot out/overlay.png
npm run verify:app -- targets                       # what pages are open
npm run verify:app --page "#/palette" -- js "…"      # any page, not just the overlay
```

`--page` names the page by url suffix, which is how the palette gets probed: it is a
second window, built the first time something asks for it, so a freshly started app
has no `#/palette` target and the error says so ("no page ending …; open targets:
…"). Exit code 0 means healthy, 1 means something was wrong with the app, 2 means the
probe itself could not run — a missing dev server should not read as a broken app.

`js` runs against the real store through the real IPC — there is no test seam inside
the app, which is the point, and also the warning: **a probe can change the user's
desktop.** Prefer read commands, make one file at a time, and put back what you
move. `floaty_install_plugin` and `floaty_rescan_plugins` reload every window, which
destroys the evaluation that awaited them: call those un-awaited and read the
outcome from `%APPDATA%\com.floaty.app\floaty.log` and from disk.

### The first-run screen

A store that has never answered the first-run question blocks (ADR 0004): the overlay
shows the folder floaty is about to adopt and what is in it, and the desktop is not
drawn until the answer comes. A probe pointed at an app in that state is looking at
the screen, not at a broken overlay — `verify:app` reports every record as missing,
because none of them is mounted yet.

`FLOATY_ROOT_CONFIRMED=1` in the app's environment answers it for that launch:

```powershell
$env:FLOATY_ROOT_CONFIRMED="1"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
npm run tauri dev
```

The answer comes from the environment and not the store, so a probe run cannot leave
the install answered: launch without the variable and the screen is still there for a
stranger. The same switch is what a developer wants when they just need the desktop up.
The log says which way it went — `first run: FLOATY_ROOT_CONFIRMED says true — the
answer comes from the environment, not the store`, or `first run: the root has not been
confirmed — the desktop waits for an answer`. `FLOATY_ROOT_CONFIRMED=0` is the other
direction: it asks for the *unconfirmed* state on a machine that has already answered,
which is how the screen itself gets probed. A probe that means to answer for good
calls `floaty_confirm_root`, which is the real thing.

The two page probes need none of this. `verify:panes` and `verify:palette` load
`settings.html` and `index.html#/palette` from the dev server with the Tauri IPC
stubbed, and neither page is the overlay, so neither mounts the guard.

### The two traps that waste the most time

**A killed app leaves its debug port alive.** A WebView2 process survives the app and
keeps serving `/json/list`: the targets are listed, the URLs are right, and every page is
a corpse — evaluating in one fails with *Cannot read properties of undefined (reading
`invoke`)*, because there is no Tauri bridge any more. `node verify/app-ready.mjs` waits
for pages that actually answer, and says which case it found; probes run against a corpse
look like app bugs.

**Vite's HMR stops reaching a long-lived overlay window.** The dev server serves the new
module and the running page keeps the old one, so a probe measures yesterday's code and
reports the fix as not working — it did, twice, and each time the "still broken" reading
was the old arithmetic. `npm run verify:app reload` reloads every overlay page; after a
*Rust* change the app restarts itself and the pages come back anyway.

## The runtime, and the world a probe runs in

`runtime.mjs` is the toolkit; `cdp.mjs` underneath it is only the transport — a target
list, one session, a key, a screenshot. The runtime owns what every probe used to write
for itself:

- **`attach()`** waits for overlay pages that are *live*. A debug port that answers is not
  a live app (the corpse trap above), so it checks `window.__TAURI_INTERNALS__` and calls
  a dead page what it is: a probe that could not run.
- **`until(what, check)`** is the only wait there is — a predicate with a deadline, never
  a stopwatch.
- **`invoke`** is the JSON-safe backend call. **`pathOf`** asks the manifest which field
  holds a record's path (never guess `target ?? path`). **`drawnBy`** answers which overlay
  page is drawing a floatie. **`takeErrors`** drains the console errors, so "no errors"
  is a property of every phase instead of a line four probes forgot. **`logLines`** reads
  the app's own log. **`mouse`/`moveTo`/`grabPoint`** drive real input.
- **`probe(name)`** owns the exit ladder: 0 healthy, 1 the app is wrong, 2 the probe could
  not run.

**A probe should not touch the user's desktop, and it no longer has to.** `Fixture` writes
a scratch world — its own data folder, its own root, its own log — and `launch()` starts the
app pointed at it with `FLOATY_DATA_DIR`, which the app resolves in one place
(`store::app_data_dir`): the store, the settings, the plugins folder, the icon cache and the
log all move together. `redo.mjs` is the reference — it merges, undoes and redoes in a folder
it made, and the user's install is byte-identical afterwards.

Two things that come with a world of your own:

- **Drive the app you started.** `launch()` refuses to run when something already answers on
  the port, because a probe that attaches to the previous run's app reports on a world it
  never made.
- **Leave nothing behind.** `stop()` kills the tree (parent first, so the children are still
  findable) and closes the sockets — an open WebSocket keeps node's event loop alive, which
  looks exactly like a hung probe.

Not every probe can have a world of its own, and the ones that cannot are worth naming. A
fixture store starts empty — a file tile, a folder tile, nothing else — so a probe whose
first act is "find the sysmon panel" finds nothing to look at, and a probe that verifies
*the gap between two screens* cannot invent the screens. `drag`, `panel-drag` and `stranded`
still need the machine's own monitors for that reason, and they are also the three that move
things a person owns. `image-thumbnail` is the fourth that touches the real desktop — it adds
only floaties it generated itself, and deletes each record on the way out — but it needs no
second screen for that. The split is deliberate: a fixture is the default, and the real desktop
is for what a directory cannot fake — two screens at different scales, a foreground fullscreen
window, the desktop window itself, the Recycle Bin, a global hotkey another app may hold.

## Writing a new probe

Start with `runtime.mjs` and `redo.mjs`; reach for `cdp.mjs` directly only for what the
runtime does not have yet. `Session.open(port, "url-suffix")`, `evaluate`, `key`,
`screenshot`, `launchChrome({ inject })`, and `errors` — the console errors and exceptions
seen since connecting, which is the signal no DOM check has.

Six rules, each of which was learned the hard way:

- **A stub must mirror the real shapes.** A field the backend returns and the stub
  omits makes a pane look fine here and break in the app, which is the one way a
  probe is worse than nothing. `panes.mjs` is the reference stub.
- **Navigate with a query as well as the hash.** `Page.navigate` between two urls
  that differ only after `#` is a *same-document* navigation, so the page is never
  rebuilt and the probe measures the previous one and reports it as stable.
- **A key that needs a default action must be a real one.** `Input.dispatchKeyEvent`
  goes through the same pipeline as the keyboard; a synthetic `KeyboardEvent` does
  not.
- **Never wait on a stopwatch; wait for the thing you are measuring.** A fixed
  `setTimeout` after `Page.navigate` measures a half-loaded page: right after the dev
  server re-transforms the module graph, four panes were reported as "drew nothing"
  with no console error and no invoke, and re-runs were green — a false failure, which
  is how a probe teaches people to ignore it. Both page probes poll for the state they
  are about to assert (the pane's groups, the palette's input) and give up after ~20s.
- **Assert the effect, not the presence.** "The pane has a button" passes on a
  button that does nothing: check that the invoke it should make was made, that the
  record changed, or that the pixels moved.
- **Give the probe a world of its own.** A probe that writes into the user's desktop,
  store or log is one bug away from damaging what it verifies, and it cannot assert
  what it left behind — the cleanup is unverifiable because the "before" was never
  known. `Fixture` + `launch()` run it against a scratch world; `redo.mjs` is the shape.

## Checking the hotkey

A global shortcut is registered with Windows, not with a page, so nothing over CDP can
press it — `Input.dispatchKeyEvent` goes to the render window. Drive it the way a
person would, and read the result back through IPC:

```powershell
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.SendKeys]::SendWait('^% ')   # ctrl+alt+space
```

Then ask: `floaty_palette_state` must report `visible: true` and `live: true`. Both
halves matter — `live` is whether Windows accepted the registration (another app may
already own the key), `visible` is whether the window actually came up.

Page probes are not a substitute for the backend tests (`cargo test`, from
`src-tauri`) or for the pure-math drivers (`scripts/physsim.js` for the physics,
`scripts/l2dcheck.js` for the live2d boundary maths — both documented in the
`floaty-widget-plugin` skill).
