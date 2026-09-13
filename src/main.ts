import "./style.css";
import { loadPlugins, pluginFor, widgetApi } from "./widgets/plugin";
import { startHeartbeat } from "./widgets/lib";

const app = document.getElementById("app");
if (app) {
  // URL looks like index.html#/note/note-1 (hash routing survives tauri custom protocol)
  const parts = window.location.hash.replace(/^#\/?/, "").split("/");
  const [kind, id] = parts;
  if (kind !== "overlay" && !id) startHeartbeat(kind === "manager" ? "manager" : `page-${kind}`);
  if (kind === "overlay") {
    import("./overlay").then((m) => m.mountOverlay(app)).catch((e) => {
      console.error("failed to mount overlay", e);
    });
  } else {
    if (id) startHeartbeat(`widget-${id}`);
    void loadPlugins().then(() => {
      const plugin = pluginFor(kind);
      if (plugin && id) plugin.mount(app, id, widgetApi);
    });
  }
  // "manager" hidden window and unknown routes intentionally render nothing
}
