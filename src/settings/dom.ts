/**
 * The element builders the settings panes share.
 *
 * Floaty ships no framework, so a pane assembles its screen from these and
 * throws the tree away when it re-renders. The row shapes are deliberately the
 * ones plugin settings code already draws (`slider-row`, `select`, `count`,
 * `folder-path`), so a plugin's own controls keep looking the way they did
 * before the shell was rewritten.
 */

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls = "",
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Coloured label for a widget kind: the plugin id, as the manifest spells it. */
export function chip(kind: string, text: string): HTMLSpanElement {
  return el("span", `kind k-${kind}`, text);
}

/** One line of explanation, below a label or a list. */
export function note(text: string): HTMLParagraphElement {
  return el("p", "set-sub", text);
}

export function emptyState(text: string): HTMLDivElement {
  return el("div", "empty", text);
}

/** A titled section: the label, an optional line about it, then a stack of cards. */
export function group(title: string, noteText?: string): { root: HTMLElement; body: HTMLElement } {
  const root = el("section", "set-group");
  root.append(el("h2", "set-group-title", title));
  if (noteText) root.append(el("p", "set-group-note", noteText));
  const body = el("div", "set-group-body");
  root.append(body);
  return { root, body };
}

/** A card. `stack` puts its contents under one another, for a block of rows. */
export function card(stack = false): HTMLDivElement {
  return el("div", stack ? "set-item stack" : "set-item");
}

/** A row of buttons that share the width. */
export function actionRow(): HTMLDivElement {
  return el("div", "set-actions");
}

/**
 * A button that runs one action and owns its own busy state, so a second click
 * cannot start the same work twice and the button says what it is doing.
 */
export function action(
  label: string,
  run: () => unknown,
  opts: { cls?: string; busyLabel?: string } = {},
): HTMLButtonElement {
  const button = el("button", opts.cls ?? "pill", label);
  button.addEventListener("click", () => {
    if (button.disabled) return;
    button.disabled = true;
    if (opts.busyLabel) button.textContent = opts.busyLabel;
    void Promise.resolve()
      .then(run)
      .catch(() => undefined)
      .finally(() => {
        button.disabled = false;
        button.textContent = label;
      });
  });
  return button;
}

/** A label with an optional line under it. Used on the left of a row. */
export function textBlock(label: string, hint?: string): HTMLElement {
  const box = el("div", "set-text");
  box.append(el("span", "set-label", label));
  if (hint) box.append(el("span", "set-sub", hint));
  return box;
}

/** Checkbox dressed as a switch, for the right-hand end of a row. The handler
 *  gets the input too, so a caller can hold it disabled while it saves. */
export function switchInput(
  checked: boolean,
  onChange: (value: boolean, input: HTMLInputElement) => void,
): HTMLLabelElement {
  const wrap = el("label", "set-switch");
  const input = el("input", "");
  input.type = "checkbox";
  input.checked = checked;
  input.addEventListener("change", () => onChange(input.checked, input));
  wrap.append(input, el("span", "set-switch-track"));
  return wrap;
}

/** A card holding one on/off setting and the sentence that explains it. */
export function toggleCard(
  label: string,
  hint: string,
  checked: boolean,
  onChange: (v: boolean) => void,
): HTMLElement {
  const item = card(true);
  const row = el("div", "set-row");
  row.append(textBlock(label, hint), switchInput(checked, onChange));
  item.append(row);
  return item;
}
