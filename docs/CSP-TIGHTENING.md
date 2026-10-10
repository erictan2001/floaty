# CSP tightening: drop `unsafe-inline` from script-src and style-src

Status: prepared, not yet verified in a running WebView2 build. Do not merge until the checklist in section (d) is complete.

Scope: `app.security.csp` in `src-tauri/tauri.conf.json` only. The string is embedded at compile time by `tauri::generate_context!`, so a rebuild is required for it to take effect.

## (a) CSP strings

Old (verbatim):

```
default-src 'self' ipc: http://ipc.localhost asset: http://asset.localhost https://asset.localhost; script-src 'self' 'unsafe-eval' 'unsafe-inline' ipc: http://ipc.localhost asset: http://asset.localhost https://asset.localhost; style-src 'self' 'unsafe-inline'; connect-src 'self' ipc: http://ipc.localhost asset: http://asset.localhost https://asset.localhost; img-src 'self' asset: http://asset.localhost https://asset.localhost data: blob:; media-src 'self' asset: http://asset.localhost https://asset.localhost data: blob:
```

New (verbatim):

```
default-src 'self' ipc: http://ipc.localhost asset: http://asset.localhost https://asset.localhost; script-src 'self' 'unsafe-eval' ipc: http://ipc.localhost asset: http://asset.localhost https://asset.localhost; style-src 'self'; connect-src 'self' ipc: http://ipc.localhost asset: http://asset.localhost https://asset.localhost; img-src 'self' asset: http://asset.localhost https://asset.localhost data: blob:; media-src 'self' asset: http://asset.localhost https://asset.localhost data: blob:
```

Changes: `'unsafe-inline'` removed from `script-src` and from `style-src`. Nothing else changed.

## (b) Evidence that the removed directives are not needed in production

`script-src 'unsafe-inline'`:
- `index.html:10` is `<script type="module" src="/src/bootstrap.ts"></script>` (external).
- `settings.html:10` is `<script type="module" src="/src/settings.ts"></script>` (external).
- No inline `<script>` bodies and no `on*=` handler attributes found in `index.html`, `settings.html`, or `src/`.
- Vite production build emits external module scripts only.

`style-src 'unsafe-inline'`:
- No `<style>` element in `src/` (grep for `<style` returns nothing).
- No `style=` attribute in `src/` markup. The `innerHTML` templates at `src/widgets/appicon.ts:56`, `src/widgets/sysmon.ts:53`, and `src/widgets/visualizer.ts:44` contain no `style=` attributes.
- `style-src` covers `<style>` elements and `style` attributes. It does not cover CSSOM writes. The ~47 `.style.<prop> = ...` assignments in `src/` (e.g. `el.style.transform = ...`) go through CSSOM and are not blocked by CSP.
- `style.css` is extracted to a `<link>` at build time (per the repo analysis), so it is covered by `'self'`.

## (c) Why `'unsafe-eval'` stays

- `node_modules/@pixi/core/dist/esm/core.mjs:5837` calls `new Function(...)` (generated uniform sync function).
- `node_modules/@pixi/core/dist/esm/core.mjs:7564` calls `new Function(...)` (generated buffer sync function).
- `src/widgets/live2d.ts:4` imports `@pixi/unsafe-eval`, which patches Pixi to use the eval-free path where possible. Pixi's core still reaches `new Function` on the paths above, so `'unsafe-eval'` is required.
- `public/live2dcubismcore.min.js` instantiates WASM via `WebAssembly.instantiate`. WASM compilation is governed by `'unsafe-eval'` in CSP (no separate `wasm-unsafe-eval` directive is set). Keeping `'unsafe-eval'` covers it.

## (d) Runtime verification checklist (human, real WebView2 on Windows, before merge)

Build a release or debug bundle with the new CSP, then:

1. Overlay window: launches and renders, no blank or unstyled UI.
2. Settings window: opens, all panes render, switching panes works.
3. DevTools console on each window: no `Refused to execute inline script` and no `Refused to apply inline style`. Any hit means a missed inline usage. Fix the source, do not re-add `'unsafe-inline'` blindly.
4. IPC: invoke a command from the UI (e.g. load settings, toggle a widget), confirm the Rust side responds and events from Rust (`listen`) arrive.
5. Live2D: load a Cubism 2 model and a Cubism 4 model; both render and play a motion. The Cubism 4 path exercises WASM.
6. Dev mode: run `npm run tauri dev`, edit a `.ts` and a `.css` file, confirm HMR updates.
   - Risk: Vite's dev client injects `<style>` elements for CSS HMR. Under the new CSP those are blocked, so CSS HMR will likely fail in dev.
   - If it fails, the options are: (1) a dev-only CSP that adds `'unsafe-inline'` to `style-src` only, applied via a separate config or `tauri dev` override, keeping the production string as written here; or (2) keep `'unsafe-inline'` in `style-src` for all builds. Option 1 is preferred. Option 2 gives up the style hardening entirely.
   - Record which path was chosen before merging.

Sign-off: all six items pass on a Windows WebView2 build, with the dev-mode outcome recorded.

## (e) Rollback

Replace the `app.security.csp` value in `src-tauri/tauri.conf.json` with the old string from section (a), then rebuild. No other file is affected.
