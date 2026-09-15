# Example plugins

Two complete plugins, written the third-party way: no floaty source, no build
step, no dependencies, and nothing but the documented `api` object. Each folder
is a plugin you can copy into floaty as it stands, and each is written to be
read.

| Example | What it is | What it is worth reading for |
| --- | --- | --- |
| [countdown](countdown) | a widget that keeps to itself: counts down to a date you pick | a whole widget in ~150 commented lines — state in the record, a self-cleaning timer, and the standard drag / menu / resize affordances |
| [trail](trail) | a widget that acts on the desktop: draw a path and the floaties line up along it | the harder half — taking the mouse away from the desktop, moving *other* widgets, and arrangements you can save and load back |

## Installing one

A plugin is two files in a folder, and installing it is a copy:

1. Tray icon → **Settings** → **Plugins** → **open plugin folder**. That is
   `%APPDATA%\com.floaty.app\plugins`.
2. Copy the example's folder in, so you end up with
   `…\plugins\countdown\plugin.json` and `…\plugins\countdown\index.js`.
3. Press **rescan plugins**. Both windows reload and the plugin appears in the
   list, with a button in **+ New floatie** (`+ countdown`, `+ trail`).

Editing a plugin is the same loop: change the files in your plugins folder and
press rescan — no restart, and a `plugin.json` you broke is reported in settings
with the reason rather than breaking the desktop.

Plugins are trusted code. They run inside floaty's own windows with the same
access floaty has, so read one before you install it — these two included.

## What they use

Neither example imports anything from floaty. Everything comes through the `api`
object `mount(root, id, api)` is handed, which is the surface documented in
[`docs/PLUGINS.md`](../../docs/PLUGINS.md):

- **State lives in the widget's own record** — `api.record.load(id)` and
  `api.record.save(rec)`. `rec.data` is the plugin's, so a countdown's date and a
  trail's saved arrangements both survive a restart without a settings file of
  their own.
- **The standard affordances** — `api.enableDrag`, `api.addPinMenu`,
  `api.addResizeHandle`: what every floatie has, added in three lines each.
- **Acting on other floaties** (trail) — `api.invoke("floaty_list")` to read the
  desktop, `api.setPos` / `api.record.save` to move an item and keep its record
  in step, and `floaty_refresh` to make the ones already on screen re-read their
  records instead of carrying on with the old ones.
- **`api.log("…")`** writes to `%APPDATA%\com.floaty.app\floaty.log`, which is the
  only way to see inside a widget that is failing quietly.

A widget cannot borrow floaty's stylesheet: an installed plugin ships its own CSS,
and both examples show that (the panel colours in their sources are floaty's
palette, written out longhand).

## Reading them

- `countdown/index.js` first: it is the shape of every plugin — mount, build the
  DOM, load the record, wire the affordances — and it documents one trap in its
  comments (a `datetime-local` field fires `change` on the first keystroke of an
  already-filled field, so "the value moved" is not "the user is finished").
- `trail/index.js` second, and only if you need a plugin that moves other
  widgets: it is long because the problem is, and every hard-won rule in it has a
  comment saying what went wrong first. Its own [README](trail/README.md)
  explains the controls; the design notes are in
  [`docs/PLUGINS.md`](../../docs/PLUGINS.md#reference-plugins).
- The built-in widgets in `src/widgets/` are the other reference: same contract,
  written in TypeScript with floaty's own helpers available.
