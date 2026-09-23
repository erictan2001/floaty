# How Floaty thinks

The ideas the rest of the app is built on. Worth ten minutes if you want to
change how Floaty behaves, or if something it does looks surprising.

## The desktop is a folder

`files_root` is the single source of truth. Every desktop item is a mirror of an
entry in it, and the backend keeps the two in step:

- A watcher (`src-tauri/src/fs_watch.rs`) does a recursive
  `ReadDirectoryChangesW` on the root — name changes only, because a desktop root
  is usually cloud-synced and size/mtime churn constantly — debounced 700 ms
  quiet / 2.5 s maximum. It reports a create, a rename (the floatie keeps its id,
  position and icon and simply moves) or a removal (the floatie leaves). A burst
  that outruns the 64 KiB change buffer is not dropped: it asks for a full
  reconcile of the root and says so in the log.
- Removal is guarded hard: records come off the desktop only when the directory
  could be read *and* every entry in it was readable, and only for entries that
  are direct children of the root. A transient read failure must not delete
  someone's icons.
- Dragging a file or directory out of a folder moves it on disk into the parent
  directory; dragging one in moves it into the folder. Both are refused when the
  source is outside the root, so the desktop can never drag files out of a folder
  it does not own.
- Grouping creates a real folder in the root (unique name, like Explorer's "New
  folder") and moves the items in. Ungrouping moves the item back out.

Captions are labels, not filenames: an app icon reads "Visual Studio Code", not
"Visual Studio Code.lnk", and the same rule applies inside a folder's grid. File
icons keep their extension, where it carries information. The stored name is
untouched — tooltips and the settings list still show the real file, and icons are
resolved from the shell and cached in the record.

## Motion and clicks

Motion is one shared setting across every desktop item, collected in the settings
window's Motion tab:

| Setting | What it does |
| --- | --- |
| `gravity`, `bounce` | how an icon falls and how it settles |
| `float_amplitude` | how far a resting icon rises, in px — the same travel in every mode |
| `float_period` | seconds per bob, in every mode |
| `float_spread` | wave only: how much of the cycle separates one icon from the next |
| `animated_ratio` | percentage of icons that float at all; the rest sit still |
| `animation_mode` | `wave` (the bob travels from icon to icon), `sync` (all together), `gentle` (a slow continuous drift) or `static` |
| `single_click`, `double_click` | `drop` / `hop` / `nothing`, and `launch` / `drop` / `nothing` |

A mode changes the *pattern*, never the height or the period: `float_amplitude` is
the travel in every mode, and `float_spread` orders the wave by where the icons
are on the desktop, so the ripple crosses the screen instead of scattering.

These apply live: a widget re-applies on the settings event, and each page fans a
single handler out to its widgets so nothing reads a half-updated cache. Every
change is logged (`[motion] …`) with the values it used and the decision each
icon made.

**The launcher.** One key — `Ctrl+Alt+Space` by default, changeable in General —
opens a search over three things at once: the applications a scan finds, the entries
in your pointed folder, and everything already floating on the desktop, so a note is
findable by name as well as a program. Type, `Enter` opens the top row, arrows move
down the list, `Escape` closes it — and a row that is a widget rather than a file
says so instead of pretending it launched something. The ranking is the part that
matters: an exact name beats a name that starts with what you typed, which beats a
match at a word boundary (typing `code` reaches *Visual Studio Code* before
*Barcode*), which beats a match inside a word, which beats initials (`vsc`). The app
list is cached for five minutes, icons are only resolved for the rows actually
shown, and the window is built once and then only shown and hidden. The tray's
**Search…** opens the same thing: if another app already owns the key you chose,
settings says so rather than leaving you with a launcher you cannot reach.

**Tidy the desktop** (General tab) lines the icons up on a grid: a pinned icon
holds the place it is given, and everything else is stacked up from the floor in
even columns — a mid-screen slot for an icon that falls would be a place it left
the moment it was mounted again. Panels and notes keep their places and the grid
lays itself around them. **Ctrl+Alt+Z** from anywhere, or the undo button in the
General tab, puts back the last change: a removed floatie, a recycled file (out of
the Recycle Bin, by name), an ungroup, a grouping, or a tidy. **Ctrl+Shift+Z** — or
the redo button beside it — replays an undone change, files and all: the same file
goes back into the same folder, and nothing else moves. Both stacks hold the last
sixteen changes of the running session; nothing is written to disk for them.

Both keys are *global*, so floaty takes them from every other application while it
is running. That is the price of the desktop never taking focus: a window that must
not steal focus cannot see a keystroke of its own, so the only way it can hear one is
to claim it from the OS. The launcher's key is configurable; these two are not.

Right-click any floatie for its menu: pin on top (a real second layer, above all
normal windows, persisted per widget), or remove it. "Stay on the desktop"
(General tab) keeps floaties visible through Win+D instead of minimising with
everything else.

## Windows and layers

- `desktop-overlay` — one full-screen transparent window holding every floatie
  that is not pinned. Click-through to the desktop is preserved by hit rectangles
  the backend keeps in sync with the frontend.
- `top-overlay` — built only while something is pinned, drawing only pinned
  records, so the desktop layer does not have to be always-on-top.
- `settings`, `manager` and `widget-<id>` — ordinary windows. The manager is a
  hidden window that outlives the overlays and carries the display/sleep watch;
  the per-widget path (`index.html#/<kind>/<id>`) still exists for spawning a
  widget as its own window.

The desktop layer is a Windows-level policy, not a Tauri flag: those windows carry
`WS_EX_TOOLWINDOW` (no taskbar button, out of alt-tab), survive `SWP_HIDEWINDOW`
from *Show desktop*, and get a 1 px recovery nudge plus a power-event pass after
sleep or display-off — the overlay is the thing that goes missing, so it is the
thing that gets repaired. The policy is gated on one predicate
(`is_desktop_layer_label`), because applying it to an ordinary window takes away
its taskbar button and breaks minimise and drag.

![Two screens, one drag: a floatie handed from one overlay to the other](../screenshots/two-screens.png)

---

[<- Back to the README](../README.md)
