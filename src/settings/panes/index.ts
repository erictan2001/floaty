import type { Pane } from "../pane";
import { desktopPane } from "./desktop";
import { addPane } from "./add";
import { motionPane } from "./motion";
import { pluginsPane } from "./plugins";
import { generalPane } from "./general";
import { diagnosticsPane } from "./diagnostics";

/**
 * The sections of the settings rail, in the order they appear. One flat list:
 * "set it up, fill the desktop, tune the motion, extend it, then diagnose it" is
 * the order a person needs them, and seven short labels need no headings of
 * their own.
 */
export const PANES: Pane[] = [
  generalPane,
  desktopPane,
  addPane,
  motionPane,
  pluginsPane,
  diagnosticsPane,
];
