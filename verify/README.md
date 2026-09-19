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
| `npm run verify:palette` | renders the launcher palette with its search stubbed, types into it, and asserts the keys do what they say | `npm run dev` |
| `npm run verify:drag` | drags a real floatie from one screen to the other with real input events, then checks it arrived, landed where the pointer held it, is drawn by exactly one window, and that no file was merged on the way | see below, two screens |

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

## Writing a new probe

`cdp.mjs` is the whole toolkit: `Session.open(port, "url-suffix")`, `evaluate`,
`invoke`, `key`, `screenshot`, `launchChrome({ inject })`, and `errors` — the console
errors and exceptions seen since connecting, which is the signal no DOM check has.

Four rules, each of which was learned the hard way:

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
