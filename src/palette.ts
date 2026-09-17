/**
 * The launcher palette: the small always-on-top window a global hotkey opens,
 * where one line of typing is the whole interface and Enter is the only button.
 *
 * The backend owns the matching — it is the side that can see the installed
 * apps, the desktop and the floaties — so this page asks on every keystroke and
 * draws whatever comes back, in the order it came back: that order is the
 * ranking, and sorting it here would throw the ranking away. An answer to a
 * keystroke the user has already typed past is dropped rather than drawn,
 * because the query it belongs to no longer exists and there is nothing to
 * patch: it is a later keystroke's answer that is wanted.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { el } from "./settings/dom";

/** One row the backend wants drawn, and the value it wants back to run it. */
export interface PaletteHit {
  kind: "app" | "file" | "folder" | "floatie";
  title: string;
  subtitle: string;
  /** "" when there is no icon at all, otherwise an http(s) url an <img> can load */
  icon: string;
  id?: string;
  path?: string;
}

/** What a run reports back: a non-empty note is the reason it did not happen. */
interface RunReport {
  note: string;
}

/** How long a keystroke waits to become a query. Long enough that a fast typist
 *  sends one call per word, short enough that the list still feels live. */
const DEBOUNCE_MS = 80;

/** How long a reason stays on screen before it stops being news. */
const NOTE_MS = 4000;

let host: HTMLElement | undefined;
let input: HTMLInputElement | undefined;
let list: HTMLElement | undefined;
let noteLine: HTMLElement | undefined;
let rows: HTMLElement[] = [];
let hits: PaletteHit[] = [];
let selected = -1;
/** Bumped by every keystroke and by every request it causes, so an answer
 *  carrying an older number can be recognised as one nobody is waiting for. */
let seq = 0;
let timer: number | undefined;
let noteTimer: number | undefined;
/** The subscription to "the backend just showed this window". */
let stopShown: (() => void) | undefined;

/** Ask the backend what matches, and drop the answer if it is out of date. */
function search(): void {
  const query = input?.value ?? "";
  const mine = ++seq;
  void invoke<PaletteHit[]>("floaty_palette_search", { query })
    .then((found) => {
      if (mine !== seq || !host?.isConnected) return;
      hits = Array.isArray(found) ? found : [];
      selected = hits.length > 0 ? 0 : -1;
      draw();
    })
    .catch((err: unknown) => {
      if (mine !== seq || !host?.isConnected) return;
      // a search that failed is a line on screen, not an exception the window
      // dies of: the palette is the only thing the user has to type into
      hits = [];
      rows = [];
      selected = -1;
      list?.replaceChildren(el("p", "palette-empty", `search failed — ${String(err)}`));
    });
}

/** Rebuild the rows. Only ever from the data: a title or a path is a string the
 *  backend chose, and it goes in as text, never as markup. */
function draw(): void {
  const box = list;
  if (!box) return;
  rows = [];
  box.replaceChildren();
  if (hits.length === 0) {
    box.append(el("p", "palette-empty", "nothing matches"));
    return;
  }
  hits.forEach((hit, index) => {
    const row = hitRow(hit, index);
    rows.push(row);
    box.append(row);
  });
  paint();
}

function hitRow(hit: PaletteHit, index: number): HTMLElement {
  const row = el("div", "palette-hit");
  row.setAttribute("role", "option");
  row.setAttribute("aria-selected", "false");
  row.append(hit.icon ? iconImage(hit) : letterTile(hit.title));
  const text = el("div", "palette-hit-text");
  text.append(el("div", "palette-hit-title", hit.title));
  text.append(el("div", "palette-hit-sub", hit.subtitle));
  row.append(text);
  row.addEventListener("click", () => {
    select(index);
    run();
  });
  return row;
}

/** The backend's icon. A url that will not load becomes the tile rather than a
 *  hole in the row, which is what a plugin with an unreadable icon looks like. */
function iconImage(hit: PaletteHit): HTMLElement {
  const img = el("img", "palette-hit-icon");
  img.alt = "";
  img.src = hit.icon;
  img.addEventListener("error", () => img.replaceWith(letterTile(hit.title)));
  return img;
}

/** A hit with no icon still needs something in the gutter — a letter reads
 *  faster than an empty box when the rows are moving under the arrows. */
function letterTile(title: string): HTMLElement {
  const letter = title.trim().slice(0, 1).toUpperCase();
  return el("span", "palette-hit-letter", letter === "" ? "?" : letter);
}

function select(index: number): void {
  if (hits.length === 0) return;
  selected = Math.min(Math.max(index, 0), hits.length - 1);
  paint();
}

/** Move the highlight. The rows are already built, so this is a class and a
 *  scroll rather than a re-render — the arrows can be held down. */
function paint(): void {
  rows.forEach((row, index) => {
    const on = index === selected;
    row.classList.toggle("selected", on);
    row.setAttribute("aria-selected", on ? "true" : "false");
  });
  // `nearest` and not `center`: a list that is already showing the row must not
  // jump under the cursor
  rows[selected]?.scrollIntoView({ block: "nearest" });
}

/**
 * Hand the selected hit to the backend and let go of the screen. Both are
 * fire-and-forget: the window is about to disappear, so waiting on either would
 * only leave a frozen row on screen. The clearing comes first because a run the
 * backend refuses leaves the note as the only thing worth reading.
 */
function run(): void {
  const hit = hits[selected];
  if (!hit) return;
  blank();
  void invoke<RunReport>("floaty_palette_run", { hit })
    .then((report) => {
      const note = report?.note ?? "";
      if (note) showNote(note);
    })
    .catch((err: unknown) => showNote(String(err)));
}

/**
 * Hide, and take the query with it. The backend shows this window again rather
 * than reopening it, so the page is never remounted — an Escape that left the
 * old query behind would hand the next summon somebody else's search.
 */
function hide(): void {
  blank();
  void invoke("floaty_palette_hide").catch(() => undefined);
}

/** Empty the palette: the query box, the rows, and any note still on screen. */
function blank(): void {
  if (input) input.value = "";
  hits = [];
  rows = [];
  selected = -1;
  list?.replaceChildren();
  if (noteTimer !== undefined) window.clearTimeout(noteTimer);
  noteTimer = undefined;
  if (noteLine) noteLine.textContent = "";
}

function showNote(text: string): void {
  if (!noteLine) return;
  noteLine.textContent = text;
  if (noteTimer !== undefined) window.clearTimeout(noteTimer);
  noteTimer = window.setTimeout(() => {
    noteTimer = undefined;
    if (noteLine) noteLine.textContent = "";
  }, NOTE_MS);
}

function onInput(): void {
  // the keystroke invalidates an answer still in flight before the debounce has
  // even decided to ask again
  seq += 1;
  if (timer !== undefined) window.clearTimeout(timer);
  timer = window.setTimeout(search, DEBOUNCE_MS);
}

function onKey(event: KeyboardEvent): void {
  if (!input?.isConnected) return;
  if (event.key === "Escape") {
    event.preventDefault();
    hide();
    return;
  }
  if (event.key === "ArrowDown") {
    event.preventDefault();
    select(selected + 1);
    return;
  }
  if (event.key === "ArrowUp") {
    event.preventDefault();
    select(selected - 1);
    return;
  }
  if (event.key === "Enter") {
    event.preventDefault();
    run();
  }
}

/** The hotkey brings this window forward rather than opening it, so the caret
 *  has to be taken back every time it arrives. */
function refocus(): void {
  input?.focus();
}

export function mountPalette(root: HTMLElement): void {
  // a remount (vite HMR, a reload while the window is up) must not leave the
  // previous page's listeners on the document
  document.removeEventListener("keydown", onKey);
  window.removeEventListener("focus", refocus);
  stopShown?.();
  stopShown = undefined;
  if (timer !== undefined) window.clearTimeout(timer);
  timer = undefined;

  host = root;
  root.innerHTML = "";

  const panel = el("div", "palette");
  const searchRow = el("div", "palette-search");
  input = el("input", "palette-input");
  input.type = "text";
  input.spellcheck = false;
  input.autocomplete = "off";
  input.placeholder = "launch an app, open a file, jump to a floatie";
  searchRow.append(input);

  list = el("div", "palette-list");
  list.setAttribute("role", "listbox");
  noteLine = el("p", "palette-note", "");
  panel.append(searchRow, list, noteLine);
  root.append(panel);

  input.addEventListener("input", onInput);
  document.addEventListener("keydown", onKey);
  window.addEventListener("focus", refocus);
  input.focus();

  // The hotkey brings this window forward rather than opening it, so nothing here
  // is remounted and the last search would still be on screen. The backend says
  // when it is summoned; this is where that search is dropped.
  void listen("floaty-palette-shown", () => {
    blank();
    refocus();
  })
    .then((unlisten) => {
      stopShown = unlisten;
    })
    .catch(() => undefined);

  // no debounce on the first ask: opening the palette shows everything, which
  // is also what an empty query means
  search();
}
