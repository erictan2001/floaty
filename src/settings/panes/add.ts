/**
 * Add: the two ways to bring something new to the desktop — an app from this
 * machine, or a folder to mirror. They are one section because they answer the
 * same question ("where do I get more floaties?") and neither is long enough to
 * deserve a rail entry of its own.
 */

import { el } from "../dom";
import type { Pane } from "../pane";
import { appsPane } from "./apps";
import { filesPane } from "./files";

export const addPane: Pane = {
  id: "add",
  label: "Add",
  title: "Add floaties",
  hint: "Float an app from this machine, or mirror a folder so its contents land on the desktop.",
  render(node) {
    const apps = el("div");
    const files = el("div");
    node.append(apps, files);
    appsPane.render(apps);
    filesPane.render(files);
  },
};
