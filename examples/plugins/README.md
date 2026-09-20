# Example plugins

Three complete plugins, written the third-party way: no floaty source, no build
step, no dependencies, and nothing but the documented `api` object. Each folder
is a plugin you can copy into floaty as it stands, and each is written to be
read.

| Example | What it is | What it is worth reading for |
| --- | --- | --- |
| [countdown](countdown) | a widget that keeps to itself: counts down to a moment you name and pick | a whole widget in ~200 commented lines — two fields edited in place (a text field and a date picker, with the focus rules between them written out), state in the record, a timer floaty owns (`api.every`), and the standard drag / menu / resize affordances |
| [trail](trail) | a widget that acts on the desktop: draw a path and the floaties line up along it | the harder half — taking the mouse away from the desktop, moving *other* widgets, and arrangements you can save and load back |

## Installing one

A plugin is two files in a folder, and installing it is a copy — either way:

- **zip it and install it**: zip the example's folder (so the zip holds
  `countdown/plugin.json`), then Tray icon → **Settings** → **Plugins** →
  **install from .zip…**. The archive is validated before it lands, and the id in
  it decides where it goes.
- **or copy the folder in**: Tray icon → **Settings** → **Plugins** → **open
  plugin folder** (that is `%APPDATA%\com.floaty.app\plugins`), copy the example's
  folder so you end up with `…\plugins\countdown\plugin.json` and
  `…\plugins\countdown\index.js`, then press **rescan plugins**.

Either way both windows reload and the plugin appears in the list, with a button
in **+ New floatie** (`+ countdown`, `+ trail`).

Editing a plugin is the same loop: change the files in your plugins folder and
press rescan — no restart, and a `plugin.json` you broke is reported in settings
with the reason rather than breaking the desktop.

Plugins are trusted code. They run inside floaty's own windows with the same
access floaty has, so read one before you install it — these three included.

## What they use

No example imports anything from floaty. Everything comes through the `api`
object `mount(root, id, api)` is handed, which is the surface documented in
[`docs/PLUGINS.md`](../../docs/PLUGINS.md):

- **State lives in the widget's own record** — `api.record.load(id)` and
  `api.record.save(rec)`. `rec.data` is the plugin's, so a countdown's date and a
  trail's saved arrangements both survive a restart without a settings file of
  their own.
- **The standard affordances** — `api.enableDrag`, `api.addPinMenu`,
  `api.addResizeHandle`: what every floatie has, added in three lines each.
- **The `"apiVersion": 2` members** (countdown) — `api.every` for a timer the
  widget owns (floaty clears it when the widget goes, so there is no reaper
  interval to write — and no timer left behind if the overlay unmounts the widget
  without telling the module). `api.after`, `api.notify` and `api.on(id, event, …)`
  are the one-shot, notification and event parts of the same set. A manifest that
  says `2` gets them; one that says `1` still loads and gets none of them.
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
