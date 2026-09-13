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

void import("./main").catch((e) => {
  report("import", String(e));
});
