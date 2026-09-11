# Creating Plugins for Floaty

Floaty is built with a modular plugin architecture that makes it easy to add new desktop widgets. Each widget is isolated in its own transparent, borderless Tauri webview window and communicates with the backend via a standardized registry.

---

## Architecture Overview

Floaty widgets have two parts:
1. **Backend Registry (`src-tauri/src/plugins.rs`)**: Defines plugin metadata, default window sizes, resizability, and default state.
2. **Frontend Plugin (`src/widgets/plugin.ts`)**: Defines the widget lifecycle (`mount`), settings controls, and per-widget management controls.

```
┌─────────────────────────────────────────────────────────────┐
│                       Rust Backend                          │
│                                                             │
│   src-tauri/src/plugins.rs                                  │
│   └── PLUGINS table: [PluginDef]                            │
│       ├── id, name, description                             │
│       ├── default_size, resizable                           │
│       └── default_data, custom_size                         │
└──────────────────────────────┬──────────────────────────────┘
                               │ IPC / Windows
┌──────────────────────────────┴──────────────────────────────┐
│                     TypeScript Frontend                     │
│                                                             │
│   src/widgets/plugin.ts                                     │
│   └── registerPlugin(FloatyPlugin)                          │
│       ├── kind, name, addLabel                              │
│       ├── mount(root, id)                                   │
│       ├── describe(record)                                  │
│       ├── renderWidgetControls(card, rec, ctx, delBtn)      │
│       └── renderSettings(card, ctx)                         │
└─────────────────────────────────────────────────────────────┘
```

---

## Step-by-Step Guide to Adding a Plugin

### 1. Register in the Rust Backend

Open [`src-tauri/src/plugins.rs`](file:///C:/Users/erict/OneDrive/Desktop/floaty/src-tauri/src/plugins.rs) and add your plugin definition to the `PLUGINS` array:

```rust
PluginDef {
    id: "counter",
    name: "Click Counter",
    description: "A simple desktop click counter.",
    default_size: (160.0, 160.0),
    resizable: false,
    default_data: || serde_json::json!({ "count": 0 }),
    custom_size: None,
},
```

#### Field Reference:
- **`id`**: Unique string identifier (e.g. `"counter"`, `"weather"`, `"timer"`).
- **`name`**: Human-readable display name shown in settings.
- **`description`**: Explanatory text shown in the Settings -> Plugins card.
- **`default_size`**: `(width, height)` in logical pixels.
- **`resizable`**: Set to `true` if the window should have native resize capability (e.g. Note, Clock).
- **`default_data`**: Closure returning initial JSON data for new widgets of this kind.
- **`custom_size`**: Optional closure `fn(base: (f64, f64), data: &Value) -> (f64, f64)` to restore dynamically sized windows from persisted data.

---

### 2. Create the Frontend Plugin Module

Create a new file under `src/widgets/<name>.ts` (for example, `src/widgets/counter.ts`):

```typescript
import { appWin, loadRecord, saveRecord, removeSelf, addPinMenu, watchPluginEnabled } from "./lib";
import type { FloatyPlugin, PluginRecord, PluginSettingsContext } from "./plugin";

export function mountCounter(root: HTMLElement, id: string): void {
  // Listen for plugin enable/disable state
  watchPluginEnabled("counter");

  const box = document.createElement("div");
  box.className = "counter-widget";

  const num = document.createElement("div");
  num.className = "count-display";

  const btn = document.createElement("button");
  btn.textContent = "+1";

  box.append(num, btn);
  root.append(box);

  // Right-click context menu (pin on top, reload, remove)
  addPinMenu(root, id);

  // Load and persist widget state
  void (async () => {
    const rec = await loadRecord(id);
    let count = typeof rec?.data["count"] === "number" ? rec.data["count"] : 0;
    num.textContent = String(count);

    btn.addEventListener("click", () => {
      count++;
      num.textContent = String(count);
      if (rec) {
        rec.data["count"] = count;
        void saveRecord(rec);
      }
    });
  })();
}

export const counterPlugin: FloatyPlugin = {
  kind: "counter",
  name: "Click Counter",
  addLabel: "+ counter", // Adds a button in Settings -> New Floatie

  mount: mountCounter,

  // Optional: summary displayed in Settings -> "On your desktop" list
  describe: (rec: PluginRecord) => {
    const c = rec.data["count"];
    return typeof c === "number" ? `count: ${c}` : undefined;
  },

  // Optional: custom controls inside the plugin's card in Settings -> Plugins
  renderSettings: (card: HTMLElement, ctx: PluginSettingsContext) => {
    // Append custom buttons, file pickers, or slider rows here
  },
};
```

---

### 3. Register the Frontend Plugin

Open [`src/widgets/plugin.ts`](file:///C:/Users/erict/OneDrive/Desktop/floaty/src/widgets/plugin.ts):

1. Import your plugin:
   ```typescript
   import { counterPlugin } from "./counter";
   ```
2. Call `registerPlugin()`:
   ```typescript
   registerPlugin(counterPlugin);
   ```
3. Add it to the exported `plugins` array for backwards compatibility.

That's it! Floaty will automatically:
- Render an enable/disable toggle in the **Settings -> Plugins** manager.
- Render a **+ counter** button in **Settings -> New floatie**.
- Route `/counter/<id>` window URLs directly to your `mount()` function.
- Display custom descriptions and controls in the **On your desktop** list.

---

## Helpful Utilities in `src/widgets/lib.ts`

When authoring widgets, [`src/widgets/lib.ts`](file:///C:/Users/erict/OneDrive/Desktop/floaty/src/widgets/lib.ts) provides standard helpers:

- **`loadRecord(id)`**: Fetches current persisted widget state (coordinates and `data` JSON).
- **`saveRecord(rec)`**: Persists updated `rec.data` or `rec.x`/`rec.y`.
- **`removeSelf(rec)`**: Closes and deletes the widget window cleanly.
- **`addPinMenu(root, id)`**: Adds standard right-click context menu (always on top toggle, reload, remove).
- **`watchSettings()`**: Listens for global settings change events (e.g. speed, bounce).
- **`watchPluginEnabled(kind)`**: Automatically hides/closes the widget if its plugin is disabled in settings.
- **`monitorArea()`**: Returns current desktop display bounds `{ x, y, w, h }` accounting for DPI scale.

---

## Advanced: Lazy-Loaded Heavy Runtimes

If your plugin uses heavy external libraries (such as PixiJS, 3D engines, or audio synthesizers), **do not statically import them in `plugin.ts` or `settings.ts`**.

Instead, follow the pattern established in [`src/widgets/live2dPlugin.ts`](file:///C:/Users/erict/OneDrive/Desktop/floaty/src/widgets/live2dPlugin.ts):
1. Keep the plugin descriptor lightweight in `<name>Plugin.ts`.
2. Inside `mount()`, dynamically import the heavy module:
   ```typescript
   mount: (root, id) => {
     void (async () => {
       const { mountHeavyRuntime } = await import("./heavyRuntime");
       mountHeavyRuntime(root, id);
     })();
   }
   ```
This ensures other widgets and the settings window remain lightweight and fast.
