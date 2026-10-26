import type { Pane } from "../pane";
import { desktopPane } from "./desktop";
import { appsPane } from "./apps";
import { filesPane } from "./files";
import { motionPane } from "./motion";
import { pluginsPane } from "./plugins";
import { generalPane } from "./general";
import { diagnosticsPane } from "./diagnostics";

/**
 * The sections of the settings rail, and the panes inside them.
 *
 * A fresh window lands on General. The groups are the questions a person
 * arrives with: "how should it behave?", "what is on my desktop?", "which
 * parts are installed/need looking at?".
 */
export const PANE_GROUPS: Array<{ label: string; panes: Pane[] }> = [
  {
    label: "Preferences",
    panes: [generalPane, motionPane],
  },
  {
    label: "Desktop",
    panes: [desktopPane, appsPane, filesPane],
  },
  {
    label: "System",
    panes: [pluginsPane, diagnosticsPane],
  },
];

/** Flat list, in rail order. The first pane is the default. */
export const PANES: Pane[] = PANE_GROUPS.flatMap((group) => group.panes);
