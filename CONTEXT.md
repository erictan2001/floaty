# Floaty - the words

The vocabulary the code and the docs are supposed to agree on. Terms only: no design, no
implementation. When a word here and a word in the code disagree, one of them is wrong and
it is worth saying so out loud.

**floatie** - anything Floaty puts on the desktop: an icon, a file, a folder, a widget. The
unit a drag moves and the undo stack records. (Code: `floaty_dropped_inner`, `rehome_stranded`.)

**widget** - a floatie that is not a file: a note, a clock, a Live2D model, a visualizer, a
system monitor. Widgets are made by Floaty; files are found in the watched folder.
(Code: `WidgetRecord`.)

**kind** - the type of a widget, the string in a record's `kind` field. Exactly the `id` of
the plugin that provides it: `note`, `clock`, `live2d`, `visualizer`, `sysmon`. (Code:
`PluginDef.id`, `BUILTINS`.)

**plugin** - a *source* of kinds. A built-in plugin ships inside Floaty; a third-party
plugin is a folder under the app-data `plugins/` directory with a `plugin.json`. A plugin
is never a thing on the desktop - the widget it creates is.

**panel** - retired. It meant "a widget with a body you can drag by", which is most of them,
so it distinguished nothing. Still in the code; not in the docs, not in new code.

**root** - the one folder Floaty watches and treats as the desktop.

**record** - everything Floaty remembers about one floatie: its `id`, its `kind`, where it sits,
and the data its kind needs. The thing a drag moves, the undo stack copies, and the store keeps.
(Code: `WidgetRecord`.)

**store** - the desktop, remembered: every record, in memory, written out to disk as one file.
Reading it is not changing it, and a record that was removed stays removed even if a window
saves it again on its way out. (Code: `store::with`, `store::StoreData`.)

**approval** - the user's agreement to run a plugin's code, recorded as the fingerprint of that
plugin's folder at the moment they agreed. Edit the files and the approval lapses until it is
given again. Built-ins are Floaty's own code and need none. (Code: `plugin_trust`,
`plugin_approved`.)

**overlay** - the transparent, full-screen, always-at-the-bottom window per monitor that
draws every floatie. One per screen; not the desktop itself.

**presence** - whether Floaty should be showing and animating right now: hidden when a
fullscreen app is in front, quiet when the machine has been idle. Decided per screen,
applied to the machine as a whole.
