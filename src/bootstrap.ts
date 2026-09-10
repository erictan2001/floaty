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

void import("./main").catch((e) => {
  report("import", String(e));
});
