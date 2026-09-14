import type { Pane } from "../pane";
import { desktopPane } from "./desktop";
import { appsPane } from "./apps";
import { filesPane } from "./files";
import { motionPane } from "./motion";
import { pluginsPane } from "./plugins";
import { generalPane } from "./general";

/**
 * The tabs of the settings window, in the order they appear.
 *
 * One pane per question the user arrives with — "what is on my desktop?", "how
 * do I add an app?", "why does it move like that?", "what is installed?" — so
 * the window never shows more than one of them at a time. The first is where a
 * fresh window lands.
 */
export const PANES: Pane[] = [desktopPane, appsPane, filesPane, motionPane, pluginsPane, generalPane];
