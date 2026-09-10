import "./style.css";
import { mountNote } from "./widgets/note";
import { mountClock } from "./widgets/clock";
import { mountPet } from "./widgets/pet";

import { mountLauncher } from "./widgets/appicon";

const app = document.getElementById("app");
if (app) {
  // URL looks like index.html#/note/note-1 (hash routing survives tauri custom protocol)
  const parts = window.location.hash.replace(/^#\/?/, "").split("/");
  const [kind, id] = parts;
  if (kind === "note" && id) mountNote(app, id);
  else if (kind === "clock" && id) mountClock(app, id);
  else if (kind === "pet" && id) mountPet(app, id);
  else if (kind === "app" && id) mountLauncher(app, id);
  // "manager" hidden window and unknown routes intentionally render nothing
}
