/**
 * Wait until the running app's pages are *live*, not just listed.
 *
 * Killing the app leaves a WebView2 process behind that still serves the debug port: the
 * targets are listed, the URLs look right, and every page is a corpse — evaluating in one
 * fails with "Cannot read properties of undefined (reading 'invoke')" because there is no
 * Tauri bridge any more. Probes have been run against that twice; this is the check that
 * tells the difference.
 *
 * Usage: node verify/app-ready.mjs [--port 9222] [--want 2] [--timeout 300]
 *   exit 0  every overlay page is live and answering
 *   exit 2  it did not come up in time (message says whether it is missing or stale)
 */
import { APP_PORT, Session, listTargets } from "./cdp.mjs";

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};
const port = Number(flag("port", APP_PORT));
const want = Number(flag("want", 2));
const timeout = Number(flag("timeout", 300));

const started = Date.now();
let stale = false;
while (Date.now() - started < timeout * 1000) {
  const targets = await listTargets(port).catch(() => []);
  const overlays = targets.filter((t) => t.type === "page" && t.url.endsWith("#/overlay"));
  if (overlays.length >= want) {
    let live = 0;
    for (let nth = 0; nth < overlays.length; nth += 1) {
      try {
        const session = await Session.open(port, "#/overlay", nth);
        const bridge = await session.evaluate("return typeof window.__TAURI_INTERNALS__");
        await session.close();
        if (bridge === "object") live += 1;
        else stale = true;
      } catch {
        stale = true;
      }
    }
    if (live >= want) {
      console.log(`app-ready: ${live} overlay page(s) live on port ${port}`);
      process.exit(0);
    }
  }
  await new Promise((r) => setTimeout(r, 3000));
}
console.error(
  stale
    ? `app-ready: port ${port} lists overlay pages but they are dead — a WebView2 left over ` +
        "from a previous run is holding the port. Kill it (or restart the app) and retry."
    : `app-ready: no live overlay pages on port ${port} after ${timeout}s`,
);
process.exit(2);
