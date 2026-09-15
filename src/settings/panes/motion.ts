/**
 * Motion: how the icons on the desktop fall, float and answer a click.
 *
 * These are the settings that used to be grafted into the app plugin's card —
 * which meant they were missing entirely whenever that card was not drawn, and
 * impossible to find if you did not know to look inside a plugin. They apply to
 * every desktop item (app icons, file icons and folders alike), so they live in
 * their own tab, one group per thing they change.
 */

import { card, group } from "../dom";
import type { Pane } from "../pane";
import { choiceRow, numberRow } from "../params";

/** A group of rows, one per setting key; unknown keys are skipped. */
function rows(title: string, note: string, keys: string[]): HTMLElement {
  const section = group(title, note);
  const item = card(true);
  for (const key of keys) {
    const row = numberRow(key) ?? choiceRow(key);
    if (row) item.append(row);
  }
  section.body.append(item);
  return section.root;
}

export const motionPane: Pane = {
  id: "motion",
  label: "Motion",
  title: "Motion & clicks",
  hint: "How the floaties move — one set of values for every icon, folder and file on the desktop.",
  render(node) {
    node.innerHTML = "";
    node.append(
      rows("Falling", "Gravity pulls a floatie back down; bounce is how much of the drop it gives back.", [
        "gravity",
        "bounce",
      ]),
      rows(
        "Floating",
        "The idle bob, one value at a time: how far a floatie rises, how long one bob takes, and — in wave mode — how far apart neighbours bob. Float height is the real travel in every mode, so the numbers mean what they say.",
        ["float_amplitude", "float_period", "float_spread"],
      ),
      rows(
        "Animation",
        "wave: a hop that travels from one floatie to the next. sync: all of them hop together. gentle: a slow, continuous drift. static: no motion at all. The last slider decides how many floaties take part — the rest sit still.",
        ["animation_mode", "animated_ratio"],
      ),
      rows("Clicking", "What one click and two clicks do to a floatie.", ["single_click", "double_click"]),
    );
  },
};
