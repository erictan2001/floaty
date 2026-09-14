# Plugins

A plugin is a widget kind: a launcher, a note, a clock, a live companion, or
whatever you write. Floaty ships nine of them, and it loads more from a folder on
disk — so you can install one somebody else wrote, or ship your own, without
touching floaty's source.

- **Installing a plugin:** [drop a folder in and press rescan](#installing-a-plugin)
- **Writing a plugin (no floaty source needed):** [the plugin format](#writing-a-plugin)
- **Writing a built-in plugin (floaty contributor):** [the two halves](#built-in-plugins)

## The two halves

Every widget kind is described in two places, split by *who needs the fact*:

| | Backend — `src-tauri/src/plugins.rs` | Frontend — `src/widgets/<name>.ts` |
| --- | --- | --- |
| owns | identity (`id`, `name`, `description`), geometry (`default_size`, `resizable`, the per-record clamp), the data a fresh widget starts with, the quick-add label, layout priority, and whether the kind stands for something on disk | behaviour: `mount`, `describe`, optional settings rows |
| needed by | the store, the hit rects, `floaty_plugins` (the manifest) | the windows that draw the widget |

The frontend reads the backend's half from the manifest (`src/widgets/pluginManifest.ts`),
so the numbers and names exist once. For built-ins that is a compile-time table;
for installed plugins it is a `plugin.json` that floaty reads at startup. Both end
up in the same manifest and behave identically — installed plugins are data, not
a special case.

## Installing a plugin

1. Open floaty's settings → **Plugins** → **open plugin folder**. That is
   `%APPDATA%\com.floaty.app\plugins`.
2. Put the plugin's folder there, so you have
   `…\plugins\countdown\plugin.json` and `…\plugins\countdown\index.js`.
3. Press **rescan plugins**. Both windows reload and the plugin shows up in the
   plugin list, with a button in **+ New floatie** if it asked for one.

A folder floaty cannot use is named in settings under the buttons, with the
reason — a broken plugin never takes the desktop down with it. The plugins folder
is read at startup and on rescan.

**Plugins are trusted code.** They run inside floaty's windows with the same
access floaty has. There is no sandbox and no marketplace: read a plugin before
you install it, exactly as you would a browser extension.

## Writing a plugin

Two files. No build step, no dependencies, no floaty source.

```text
%APPDATA%\com.floaty.app\plugins\countdown\
  plugin.json     what the desktop needs to know about the widget
  index.js        the widget itself (an ES module)
```

### `plugin.json`

```json
{
  "id": "countdown",
  "name": "Countdown",
  "description": "Counts down to a date you pick.",
  "version": "1.0.0",
  "author": "you",
  "apiVersion": 1,
  "entry": "index.js",
  "size": { "w": 240, "h": 140 },
  "resizable": true,
  "minSize": { "w": 170, "h": 96 },
  "maxSize": { "w": 620, "h": 400 },
  "defaultData": { "target": "" },
  "addLabel": "+ countdown",
  "layoutPriority": 6
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Unique kind name: 2–32 chars of `a-z`, `0-9`, `-`, `_`. Must not clash with a built-in (`note`, `clock`, `pet`, `app`, `file`, `folder`, `live2d`, `visualizer`, `sysmon`). Widget records are named `<id>-<n>`. |
| `name` | yes | Shown in the settings list. |
| `description` | no | Shown under the name. |
| `version`, `author` | no | Shown in the settings list. |
| `apiVersion` | yes | Must be `1`. How a future format change stays honest. |
| `entry` | no | Module file, default `index.js`. Must stay inside the plugin folder. |
| `size` | yes | Size a fresh widget gets, in logical pixels (60–4000). |
| `resizable` | no | `true` lets the user drag a size grip; `false` pins it to `size`. |
| `minSize`, `maxSize` | no | Clamp for a resizable widget (defaults 80×60 … 2000×2000). |
| `defaultData` | no | The `data` a new widget record starts with (an object, yours to use). |
| `addLabel` | no | Button text in **+ New floatie** (`"+ countdown"`). Omit for no button. |
| `layoutPriority` | no | Where the desktop layout puts it, lower first. Built-ins: live2d 1, note 2, clock 3, pet 4, visualizer/sysmon 5, icon grid 10. |
| `desktopItem` | no | Only for a widget that stands for a real file: `{ "pathKey": "target", "noun": "… floatie", "group": false }` gives it the shell verbs and a Recycle Bin delete. Such items are mirrored from the root folder, so they follow the disk (see [the desktop as a mirror](#desktop-items)) — and group them and the real files move into a new folder in the root, an app getting a shortcut written instead of having its program moved. Label them with `api.displayName(name)` so a caption reads `Arc`, not `Arc.lnk`. |

### `index.js`

Export a default object. `mount` is the only required member.

```js
export default {
  // root: an empty container sized like the widget. id: this widget's record id.
  // api: the supported surface for talking to floaty (table below).
  async mount(root, id, api) {
    const rec = await api.record.load(id);
    const box = document.createElement("div");
    box.textContent = `hello from ${id}`;
    box.style.cssText = "width:100%;height:100%;display:grid;place-items:center;color:#fff";
    root.append(box);

    api.enableDrag(box, rec);       // drag the widget by `box`, position saved
    api.addPinMenu(box, () => rec); // right-click menu with the standard rows
    api.log(`${id} mounted`);       // goes to floaty.log
  },

  // one line next to the id in settings (optional)
  describe(rec) {
    return "a short summary";
  },

  // extra controls in the widget's card in the settings window's Desktop tab
  // (optional — the delete button is passed so you can slot yours in front of it)
  renderWidgetControls(card, rec, ctx, deleteBtn) {
    // ctx.safe("label", () => api.invoke(...)), ctx.refreshWidgets(), ctx.showError(msg)
  },

  // extra rows under the plugin's card in the settings window's Plugins tab
  renderSettings(card, ctx) {
    // ctx.getSettings(), ctx.getSharedRow("pet_speed"), ctx.updateSettings(patch),
    // ctx.safe(...), ctx.showError(...), ctx.refreshWidgets()
  },
};
```

Settings live in one of two places, and a plugin's own card is for settings only
*it* has (a pet's speed, a visualizer's gain, the folder it scans). A setting that
applies to every desktop item — gravity, bounce, the float parameters, click
behaviour — belongs in the **Motion tab**, which is where the built-ins keep
theirs. `ctx.getSharedRow(name)` still hands you a row bound to one of those
names (`gravity`, `pet_speed`, …) if you want it in your own card too; it returns
a fresh row each call, and `undefined` for a name this build does not have.

To follow a setting while the widget is on screen, pass `api.onSettings(cb)` the
function that re-applies it: `api.onSettings(draw)` runs `draw` on every change
and once as soon as the settings are loaded, after floaty has updated its cache.
Do not register your own `listen("floaty-settings-changed")` for this — that puts
your handler in a race with floaty's cache updater, and yours can run with the
values from before the change. `api.settings()` reads the current values at any
time.

### The `api` object

Everything a plugin needs from floaty. Use this instead of importing floaty's
own modules: this surface stays stable, floaty's internals do not.

| Member | What it does |
| --- | --- |
| `api.invoke(cmd, args)` | Call any backend command, e.g. `floaty_list`, `floaty_save`, `floaty_scan_apps`. `floaty_refresh` takes `{ ids }` and remounts those widgets: the way to make floaties you moved or changed from your own widget notice their new records. |
| `api.log(msg)` | Append to `%APPDATA%\com.floaty.app\floaty.log`. The only way to see inside the overlay — use it while developing. |
| `api.record.load(id)` | The widget's record: `id`, `kind`, `x`, `y`, `data`. |
| `api.record.save(rec)` | Persist it. `data` is yours. |
| `api.enableDrag(el, rec, opts?)` | Make `el` drag the widget around (whole surface, or a title bar) **and keep the position saved**. A press only becomes a drag after ~4px of travel, so clicks still reach your own handlers, and buttons/inputs/the resize grip keep working. |
| `api.setPos(id, x, y)` | Move the widget (keeps the desktop's hit rects in step). |
| `api.setSize(id, w, h)` | Resize the widget's slot. |
| `api.addPinMenu(wrap, getRec, opts?)` | The standard right-click menu for this widget. `opts.rows(api)` adds the plugin's own rows above the standard ones — `row`, `run`, `divider`, `note`, `close`, and `swap` to draw a list in place of the commands (how the live2d model picker works). |
| `api.addResizeHandle(wrap, rec, minW, minH)` | Bottom-right grip that resizes and saves the record. |
| `api.removeSelf(rec)` | Remove the widget, asking first when the user enabled confirmation. |
| `api.settings()` | Current global settings (`gravity`, `bounce`, `pet_speed`, …). |
| `api.onSettings(cb)` | Run `cb` on every settings change, and once when the settings are first loaded. This is how a widget follows a setting — see [the note above](#indexjs). |
| `api.watchSettings()` | Make sure the settings have been loaded once. `settings()` reads whatever is cached, and this reports nothing back to you. |
| `api.displayName(name)` | The desktop's caption rule: `Arc.lnk` → `Arc`, while a file keeps its extension. Use it for any label that names a file. |
| `api.monitorArea()` | The desktop area the widget may use: `{ x, y, w, h }`. |

Style the widget yourself: size the container to `100%` and inject a `<style>`
element from your module. To look like the built-in panels, build your card from
the values the panels use — `linear-gradient(160deg, rgba(20,18,38,.84),
rgba(28,24,52,.74))`, `1px solid rgba(255,255,255,.13)`, `16px` radius,
`var(--card-shadow)` for the elevation, `#ece9ff` text with `tabular-nums`, small
caps captions at 55% opacity — or use the `--panel-bg` / `--panel-border` /
`--panel-radius` / `--panel-ink*` variables on `:root` (a built-in does; an
installed plugin should inline the values, since floaty's stylesheet is not part
of the plugin contract).

### Debugging

- `api.log("…")` → `%APPDATA%\com.floaty.app\floaty.log`.
- A plugin that fails to import is logged (`[plugin] <id> failed to load: …`) and
  skipped; a `mount()` that throws is logged too (`[overlay] mount <id> failed: …`).
- Edit the files, press **rescan plugins**, and both windows reload — no restart.
  A `plugin.json` you broke is reported by name in settings.
- A settings edit logs what each window received and what each widget did with it
  (`[motion] desktop-overlay settings: ratio=0 …`, `[motion] app-17 -> static …`),
  so "it only took effect after a restart" can be read out of the log instead of
  guessed at.

### Desktop items

A widget whose manifest carries `desktopItem` is not free-floating: its record
mirrors an entry in the root folder floaty is pointed at. `pathKey` names the field
that holds the path (the frontend's `desktopItemFor(kind)` tells you whether a kind
is one of these), `noun` is what the removal dialog calls it, and `group: true`
means other items can be dropped into it.

The root is watched, so an item added, renamed or deleted on disk appears, moves or
leaves on the desktop while floaty runs — a rename keeps the floatie's id,
position and icon. Removal is only ever applied to entries that are direct children
of the root, and only when the whole directory could be read. If you write a kind
of this sort, expect `rec.data` to be reconciled with the disk rather than owned by
your widget.

### Reference plugins

[`examples/plugins/countdown`](../examples/plugins/countdown) is a complete,
commented plugin: it saves state in the record, redraws every second, and wires up
dragging, the right-click menu and the resize grip — with no dependencies. Copy it into your
plugins folder as a starting point.

[`examples/plugins/trail`](../examples/plugins/trail) is the other half of the
picture, for a plugin that acts on the desktop instead of sitting on it: it resizes
its own slot to the monitor area to take the mouse (the overlay is click-through
outside a widget's own rectangle), captures strokes on a canvas, then moves the
*other* floaties with `floaty_list`, `api.setPos` and `api.record.save` — and calls
`floaty_refresh` so the ones already on screen re-read their records instead of
carrying on falling. Its two modes are the interesting part: **single** treats each
stroke as an instruction and arranges at once, staying open so the next stroke moves
them again; **multiple** accumulates paths and arranges only when the user presses
done. Enter leaves either mode (in multiple it arranges first), and escape does the
same without a word about cancelling. A mode's surface is its own thing, not the widget
stretched over the desktop: the panel hands its slot the monitor area so the mouse
reaches the drawing, hides itself (header, hint, count, buttons) for as long as the mode
is open, and comes back exactly as it was. Four things it shows that are worth copying:
hang the canvas off `document.body`, or a slot can clip it; clear the canvas before
filling it, or a see-through fill composites over the opaque one from the last paint
and stays opaque; keep a mode's own state (`session`) apart from the saved state until
the user confirms it, so escape really is a cancel; and space a run of icons by **one gap
for every pair** — an even split of the path — rather than giving each pair exactly the
room its local direction demands. The latter is defensible pair by pair and looks wrong:
a path that steepens then flattens comes out bunched where it steepened and sparse where
it flattened. A run also has the two end margins of slack to slide in, and sliding it
whole keeps every gap identical, so pick the phase that clashes least with trails already
arranged before moving any single icon; that, and nudging only icons that land on
*another* trail, is what keeps a crossing from deforming an otherwise even run. The
icons are then shared between the trails **by length, not by count** — a short trail
given as many icons as a long one is cramped while the long one is sparse, and the
spacing either side of a crossing stops matching; what is wanted is the same gap on every
trail, and that gap is (total length)/(icons), so each trail's share of the icons is its
share of the length. Largest-remainder rounding, a floor of one icon per trail while
there are icons to spare, and membership decided by which trail an icon is nearest to
(so nothing crosses the desktop to reach its place). Each icon it places is marked
`arranged` and `pinned` on its own record, which is what makes an arrangement survive a
restart: the overlay's layout pass leaves any record that has a position exactly where it
is — off the edge of the screen and overlapping a neighbour included, because the user put
it there — and only gives a place of its own to a record the backend has never placed
(one written at 0,0), which is how a freshly scanned icon still lands somewhere clean.

## Built-in plugins

Same contract, but shipped inside floaty. Adding one is three edits:

1. **`src-tauri/src/plugins.rs`** — add a `PluginDef`:

   ```rust
   PluginDef {
       id: "counter",
       name: "Click counter",
       description: "A desktop click counter.",
       default_size: (160.0, 160.0),
       resizable: false,
       default_data: || serde_json::json!({ "count": 0 }),
       custom_size: None,
       desktop_item: None,
       add_label: Some("+ counter"),
       layout_priority: 10,
   },
   ```

   `custom_size` is the per-record clamp used by resizable widgets
   (`fn(base, data) -> (w, h)`, usually `size_from_wh(base, data, min, max)`).
   Add the id to the `KINDS` list in the tests.

2. **`src/widgets/counter.ts`** — the module, exporting a `FloatyPlugin`:

   ```ts
   import { loadRecord, saveRecord, addPinMenu, addResizeHandle, watchPluginEnabled } from "./lib";
   import type { FloatyPlugin } from "./plugin";

   export const counterPlugin: FloatyPlugin = {
     kind: "counter",
     async mount(root, id) {
       watchPluginEnabled("counter");
       const rec = await loadRecord(id);
       // ... build the DOM, then addPinMenu(root, () => rec)
     },
     describe: (rec) => `count: ${String(rec.data["count"] ?? 0)}`,
   };
   ```

   No `name`, size or label here — the manifest owns those.

3. **`src/widgets/plugin.ts`** — import it and add it to `BUILTINS`. The registry,
   `pluginFor`, `allPlugins` and `layoutPriority` derive from that one list.

Then `npm run build` (which runs `node scripts/check-plugins.mjs`) and add the CSS
block plus a `.k-<kind>` chip colour in `src/style.css`. The check script fails
when the two id lists disagree or when a JSON field the frontend reads was
renamed — that is the guard rail the old two-registry setup was missing.

**Never keep a kind list anywhere else.** Core code asks the manifest instead:
`pathOf(rec)`, `desktopItemFor(kind)`, `pluginSize(rec)`, `layoutPriorityFor(kind)`,
`isDesktopItem`. Testing `kind === "folder"` in shared code is how a new kind
silently loses the shell verbs.

### Lazy-load heavy runtimes

Do not statically import pixi, a 3D engine or an audio stack in a plugin
descriptor or in settings. Follow `src/widgets/live2dPlugin.ts`: keep the
descriptor light and `await import("./heavy")` inside `mount()`, so the overlay
and settings window stay cheap for every other widget.

### Standard helpers in `src/widgets/lib.ts`

Built-in plugins may import these directly (third-party plugins get the equivalent
through `api`):

- **Records and settings** — `loadRecord`, `saveRecord`, `currentSettings`,
  `onSettings`, `refreshSettings`, `watchSettings`, `watchPluginEnabled`. Follow a
  setting with `onSettings(cb)`, never with your own settings listener.
- **Position and size** — `logicalPos`, `setLogicalPos`, `setWidgetPos`,
  `setWidgetSize`, `monitorArea`, `trackPosition`, `overlaySlots`,
  `isOverlayMode`, `isTopLayer`, `enforceDesktopLayer`.
- **Interaction** — `enableOverlayDrag`, `addPinMenu`, `addResizeHandle`,
  `removeSelf`, `confirmRemoveDialog`, `describeForConfirm`, `makeBar`, and
  `notifyDragging` + `notifyDragMove`, which arm the desktop's drop and merge
  preview while an icon is dragged by hand.
- **Presentation** — `applyFloatieAnimation` (the shared motion modes),
  `iconIsMissing` (whether an icon URL still needs resolving) and `displayName`
  (the caption rule, so a label that names a file matches its neighbours).

Kind facts come from the manifest instead: `pathOf`, `desktopItemFor`,
`pluginSize`, `isDesktopItem` and `layoutPriorityFor` in
`src/widgets/pluginManifest.ts`.
