import "./style.css";
import { plugins } from "./widgets/plugin";

const app = document.getElementById("app");
if (app) {
  // URL looks like index.html#/note/note-1 (hash routing survives tauri custom protocol)
  const parts = window.location.hash.replace(/^#\/?/, "").split("/");
  const [kind, id] = parts;
  if (kind === "overlay") {
    import("./overlay").then((m) => m.mountOverlay(app)).catch((e) => {
      console.error("failed to mount overlay", e);
    });
  } else {
    const plugin = plugins.find((p) => p.kind === kind);
    if (plugin && id) plugin.mount(app, id);
  }
  // "manager" hidden window and unknown routes intentionally render nothing
}
