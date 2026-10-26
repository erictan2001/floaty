/**
 * The launcher key's capture, as a plain function over a keyboard event.
 *
 * The backend parses accelerators with `global-hotkey`, which knows names like
 * `Super`, `ArrowLeft` and `PageDown` but nothing about a browser's `event.key`
 * (`" "`, `"ArrowLeft"`, `"Win"`). This module is the one translation: physical
 * codes in, a string the backend accepts out. It is separate from the pane so the
 * translation can be tested without a window.
 */

export interface KeyPress {
  /** `KeyboardEvent.code`: a physical key, so the layout does not change it. */
  code: string;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
}

export type Capture =
  { ok: true; accelerator: string } | { ok: false; reason: "unsupported" | "modifier" };

/**
 * Every non-modifier key the backend's parser knows. Letters and digits use the
 * parser's short forms; punctuation uses its names, because `event.code` has no
 * stable one-character spelling for a keyboard layout.
 */
const KEYS: Record<string, string> = {
  Space: "Space",
  Tab: "Tab",
  Enter: "Enter",
  Backspace: "Backspace",
  Delete: "Delete",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Insert: "Insert",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  ArrowUp: "Up",
  ArrowDown: "Down",
  Backquote: "Backquote",
  Minus: "Minus",
  Equal: "Equal",
  BracketLeft: "BracketLeft",
  BracketRight: "BracketRight",
  Backslash: "Backslash",
  Semicolon: "Semicolon",
  Quote: "Quote",
  Comma: "Comma",
  Period: "Period",
  Slash: "Slash",
  PrintScreen: "PrintScreen",
  Pause: "Pause",
  NumpadAdd: "NumAdd",
  NumpadSubtract: "NumSubtract",
  NumpadMultiply: "NumMultiply",
  NumpadDivide: "NumDivide",
  NumpadDecimal: "NumDecimal",
  NumpadEnter: "NumEnter",
  NumpadEqual: "NumEqual",
};

for (const letter of "ABCDEFGHIJKLMNOPQRSTUVWXYZ") KEYS[`Key${letter}`] = letter;
for (let digit = 0; digit <= 9; digit += 1) {
  KEYS[`Digit${digit}`] = String(digit);
  KEYS[`Numpad${digit}`] = `Num${digit}`;
}
for (let fn = 1; fn <= 12; fn += 1) KEYS[`F${fn}`] = `F${fn}`;

/** Is this keydown a modifier being held down, not a shortcut on its own? */
export function isModifierCode(code: string): boolean {
  return /^(Control|Alt|Shift|Meta|OS)(Left|Right)$/.test(code);
}

/**
 * The accelerator a keypress means, or why it cannot be one.
 *
 * A modifier is required: a bare letter or F-key would take that key away from
 * every application, so the capture refuses it rather than letting a slip of the
 * finger become a machine-wide binding.
 */
export function acceleratorFrom(press: KeyPress): Capture {
  const key = KEYS[press.code];
  if (!key) return { ok: false, reason: "unsupported" };
  if (!press.ctrl && !press.alt && !press.meta) return { ok: false, reason: "modifier" };
  const parts: string[] = [];
  if (press.ctrl) parts.push("Ctrl");
  if (press.alt) parts.push("Alt");
  if (press.shift) parts.push("Shift");
  if (press.meta) parts.push("Super");
  parts.push(key);
  return { ok: true, accelerator: parts.join("+") };
}

/** What a stored accelerator looks like to a person. `Super` is the Windows key. */
export function prettyAccelerator(text: string): string {
  return text
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => {
      switch (part.toLowerCase()) {
        case "control":
          return "Ctrl";
        case "super":
        case "cmd":
        case "command":
          return "Win";
        case "option":
          return "Alt";
        case "commandorcontrol":
        case "commandorctrl":
        case "cmdorcontrol":
        case "cmdorctrl":
          return "Ctrl";
        case "escape":
          return "Esc";
        default:
          return part;
      }
    })
    .join(" + ");
}
