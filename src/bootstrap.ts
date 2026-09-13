import { invoke } from "@tauri-apps/api/core";

function report(kind: string, msg: string): void {
  invoke("floaty_log", { msg: `[webview ${window.location.hash}] ${kind}: ${msg}` }).catch(
    () => undefined,
  );
}

window.addEventListener("error", (e) => {
  report("error", `${e.message} @${e.lineno}:${e.colno}`);
});
window.addEventListener("unhandledrejection", (e) => {
  report("rejection", String((e as PromiseRejectionEvent).reason));
});

import { listen } from "@tauri-apps/api/event";

listen("floaty-reload", () => {
  window.location.reload();
}).catch(() => undefined);

if (import.meta.hot) {
  import.meta.hot.accept(() => {
    window.location.reload();
  });
}
// reload trigger: v21

// One line per page load saying where the browser's time went. In a dev build
// Vite serves every module as its own request, so this is the number that
// explains a slow reload; in a production build it should be a handful of files.
const boot = performance.now();
const nav = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
const res = performance.getEntriesByType("resource") as PerformanceResourceTiming[];
const heavy = [...res]
  .sort((a, b) => b.duration - a.duration)
  .slice(0, 6)
  .map((r) => `${r.name.split("/").slice(-2).join("/")} ${Math.round(r.duration)}ms`)
  .join(", ");
report(
  "boot",
  `scripts at ${Math.round(boot)}ms (dom ${Math.round(nav?.domContentLoadedEventEnd ?? 0)}ms, load ${Math.round(
    nav?.loadEventEnd ?? 0,
  )}ms); requests ${res.length} totalling ${Math.round(res.reduce((a, r) => a + r.duration, 0))}ms; slowest: ${heavy}`,
);

void import("./main").catch((e) => {
  report("import", String(e));
});
