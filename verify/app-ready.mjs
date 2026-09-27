/**
 * Wait until the running app's pages are *live*, not just listed.
 *
 * Killing the app leaves a WebView2 process behind that still serves the debug port: the
 * targets are listed, the URLs look right, and every page is a corpse — evaluating in one
 * fails with "Cannot read properties of undefined (reading 'invoke')" because there is no
 * Tauri bridge any more. Probes have been run against that twice; this is the check that
 * tells the difference.
 *
 * That check is `attach()` in `runtime.mjs` now, and every probe that needs a live app
 * gets it by calling it — so this file is the smallest honest demonstration of it: a
 * runnable script that waits, says what it found, and leaves with a code that means what
 * it says.
 *
 * Usage: node verify/app-ready.mjs [--port 9222] [--want 2] [--timeout 300]
 *   exit 0  every overlay page is live and answering
 *   exit 2  it did not come up in time (message says whether it is missing or stale)
 */
import { APP_PORT } from "./cdp.mjs";
import { attach, flag } from "./runtime.mjs";

const port = Number(flag("port", APP_PORT));
const want = Number(flag("want", 2));
const timeout = Number(flag("timeout", 300));

const app = await attach({ port, screens: want, timeout }).catch((error) => {
  // The runtime's message already says which of the two cases this is, and telling
  // them apart is the whole reason the wait exists: a corpse and an app that never
  // started need different advice.
  console.error(`app-ready: ${error.message}`);
  process.exit(2);
});

console.log(`app-ready: ${app.sessions.length} overlay page(s) live on port ${port}`);
await app.close();
process.exit(0);
