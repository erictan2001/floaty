/**
 * Render the launcher palette for real, with the search backend stubbed, and
 * drive it from the keyboard: type at it, narrow it, arrow through it, run a row
 * and dismiss it.
 *
 * Run with the Vite dev server up (any port; pass it as the first argument):
 *
 *     npm run dev                        # in one terminal, :1420
 *     npm run verify:palette             # in another
 *
 * Why this exists: the palette is a plain webview page whose whole interface is
 * the keyboard, so every behaviour that matters — which row is selected, what
 * Enter hands to the backend, whether a keystroke past the debounce paints a
 * stale answer — can be exercised without the app. What `tsc` cannot see is a
 * key that never reaches the handler, an input that lost the caret (the window
 * is shown again rather than remounted, so it is never re-autofocused), or a
 * list that quietly stops rendering after a failed search.
 *
 * The two rules from verify/README.md that decide how this is written: the stub
 * mirrors the real shapes (a hit carries every field the backend sends, and an
 * icon is a real url the page loads), and the keys are *real* key events, so the
 * arrows go through the same pipeline as the user's own.
 */

import path from "node:path";
import { fileURLToPath } from "node:url";
import { launchChrome } from "./cdp.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const base = (process.argv[2] ?? "http://localhost:1420").replace(/\/$/, "");
const shiny = process.argv.includes("--shot");

/**
 * What the backend answers for the queries this probe types at it. Four kinds,
 * so every row shape is drawn, and two with no icon: the letter tile is what a
 * real library shows the moment one app's icon cannot be read, and a stub of
 * nothing but urls would never draw that branch. The icon is a file the dev
 * server really serves — a 404 here would be this probe failing its own page.
 */
const HITS = [
  { kind: "app", title: "Arc", subtitle: "C:\\Arc.lnk", icon: `${base}/assets/icon.png` },
  { kind: "file", title: "notes.txt", subtitle: "C:\\Users\\erict\\notes.txt", icon: "" },
  { kind: "folder", title: "Games", subtitle: "C:\\Games", icon: `${base}/assets/icon.png` },
  { kind: "floatie", title: "System monitor", subtitle: "floatie on the desktop", icon: "" },
];

/**
 * The stub. It matches the way the real search does — an empty query is the
 * whole library, anything else is a substring of the row's own text — so that
 * "typing narrows the list" is the page asking the backend again, not the page
 * filtering what it already had.
 */
export const STUB = `(() => {
  const HITS = ${JSON.stringify(HITS)};
  const answer = async (cmd, args) => {
    switch (cmd) {
      case "floaty_palette_search": {
        const q = String(args?.query ?? "").trim().toLowerCase();
        if (!q) return HITS;
        return HITS.filter((h) => (h.title + " " + h.subtitle).toLowerCase().includes(q));
      }
      case "floaty_palette_run": return { note: "" };
      case "floaty_palette_hide": return null;
      case "plugin:event|listen": return 1;
      case "plugin:event|emit": return null;
      case "plugin:event|emit_to": return null;
      case "floaty_log": return null;
      case "floaty_heartbeat": return null;
      default: return null;
    }
  };
  const callbacks = new Map();
  let nextCb = 1;
  // every call with its arguments, not just its name: what a run was handed is
  // the assertion, and the command name alone cannot say it
  window.__CALLS__ = [];
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "palette" }, currentWebview: { label: "palette" } },
    transformCallback: (cb) => { const id = nextCb++; callbacks.set(id, cb); return id; },
    unregisterCallback: (id) => callbacks.delete(id),
    convertFileSrc: (p) => "http://asset.localhost/" + encodeURIComponent(p),
    invoke: async (cmd, args) => { window.__CALLS__.push({ cmd, args }); return answer(cmd, args); },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
})()`;

/** Everything one check needs to look at, read in the page and returned whole. */
const MEASURE = `(() => {
  const rows = [...document.querySelectorAll(".palette-hit")];
  const box = document.querySelector(".palette-list");
  const selected = rows.findIndex((r) => r.getAttribute("aria-selected") === "true");
  const hit = rows[selected];
  const inside = (() => {
    if (!hit || !box) return false;
    const a = hit.getBoundingClientRect();
    const b = box.getBoundingClientRect();
    return a.top >= b.top - 1 && a.bottom <= b.bottom + 1;
  })();
  return {
    input: document.querySelector(".palette-input")?.value ?? null,
    focused: document.querySelector(".palette-input") === document.activeElement,
    titles: rows.map((r) => r.querySelector(".palette-hit-title")?.textContent ?? ""),
    selected,
    marked: rows.filter((r) => r.classList.contains("selected")).length,
    icons: rows.filter((r) => r.querySelector("img.palette-hit-icon")).length,
    letters: rows.filter((r) => r.querySelector(".palette-hit-letter")).length,
    inView: inside,
    empty: document.querySelector(".palette-empty")?.textContent ?? null,
    note: document.querySelector(".palette-note")?.textContent ?? "",
    listScrolls: box ? getComputedStyle(box).overflowY === "auto" : false,
    overflowX: document.documentElement.scrollWidth - window.innerWidth,
    overflowY: document.documentElement.scrollHeight - window.innerHeight,
    calls: window.__CALLS__,
  };
})()`;

/** Console noise that is not the app's fault and would fail every run. */
const IGNORE = [/favicon\.ico/];

const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const of = (view, cmd) => view.calls.filter((c) => c.cmd === cmd);
const lastSearch = (view) => of(view, "floaty_palette_search").at(-1);
const quiet = (session) => session.errors.filter((e) => !IGNORE.some((p) => p.test(e)));

/** Just enough of the page to know whether it has become the palette yet. */
const SETTLE = `(() => ({
  href: location.href,
  ready: document.readyState,
  stubbed: typeof window.__TAURI_INTERNALS__ === "object",
  mounted: !!document.querySelector(".palette-input"),
  calls: (window.__CALLS__ ?? []).length,
}))()`;

let failed = 0;
function check(name, ok, detail = "") {
  if (!ok) failed++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${ok || !detail ? "" : ` — ${detail}`}`);
}

// The dev server has to answer before anything else is measured: pointed at a dead
// port, a probe renders blank panes and reports them as broken, when the truth is
// that there was nothing to look at. Exit 2 for that, and 1 for a page that drew
// wrong.
try {
  const alive = await fetch(base);
  if (!alive.ok) throw new Error(`HTTP ${alive.status}`);
} catch (err) {
  console.error(
    `verify: could not reach ${base} — is \`npm run dev\` running?\n  ${err.message}`,
  );
  process.exit(2);
}

// Nothing to look at is not the same as an app that is broken: exit 2 when the dev
// server (or Chrome) cannot be reached, so a failed probe never reads as a failed app.
let chrome;
try {
  chrome = await launchChrome({ inject: STUB, window: "560,380" });
} catch (err) {
  console.error(
    `verify: could not open ${base} — is \`npm run dev\` running?\n  ${err.message}`,
  );
  process.exit(2);
}
const session = chrome.session;

/**
 * One real key press, through the same input pipeline as the user's keyboard — a
 * synthetic `KeyboardEvent` dispatched in the page is not trusted, and the arrows
 * only work if they are.
 */
function press(combo) {
  return session.key(combo);
}

/**
 * Wait for the page to *be* the palette. A stopwatch will not do: on a machine
 * that is also building the app, a just-started Chrome can sit at
 * `readyState: interactive` for seconds with the module graph still coming down
 * the wire, and measuring then reports every check against a half-loaded
 * document. If nothing has run by the deadline the load itself was lost, which
 * has been seen and is not the page's doing — so it is sent once more, in the
 * open, and the checks are left to fail on their own if it happens twice.
 */
async function settle(seconds = 20) {
  const look = async (deadline) => {
    let state = await session.evaluate(SETTLE);
    while (!state.mounted && Date.now() < deadline) {
      await wait(150);
      state = await session.evaluate(SETTLE);
    }
    return state;
  };
  const first = await look(Date.now() + seconds * 1000);
  if (first.mounted) return first;
  console.log(
    `     the load never arrived (page=${first.ready}, stub=${
      first.stubbed ? "in place" : "missing"
    }, calls=${first.calls}) — navigating again`,
  );
  await session.send("Page.navigate", { url: `${base}/index.html?palette=retry#/palette` });
  return await look(Date.now() + seconds * 1000);
}

try {
  // A query as well as the hash: `Page.navigate` between two urls that differ
  // only after `#` is a *same-document* navigation, so the page would never be
  // rebuilt and the probe would measure whatever was there before it.
  await session.send("Page.navigate", { url: `${base}/index.html?palette=1#/palette` });
  const settled = await settle();

  const opened = await session.evaluate(MEASURE);
  console.log(`     ${JSON.stringify(opened.calls.map((c) => c.cmd))}`);
  check(
    "the palette opened on a focused search box",
    opened.input === "" && opened.focused,
    `value=${JSON.stringify(opened.input)} focused=${opened.focused} ` +
      `page=${settled.ready} stub=${settled.stubbed ? "in place" : "MISSING"}`,
  );
  check(
    `an empty query draws every stubbed hit, in the backend's order (${HITS.length})`,
    opened.titles.join(" | ") === HITS.map((h) => h.title).join(" | "),
    opened.titles.join(" | "),
  );
  check(
    "the first ask was the empty query",
    lastSearch(opened)?.args?.query === "",
    JSON.stringify(lastSearch(opened) ?? null),
  );
  check(
    "the first row starts selected",
    opened.selected === 0 && opened.marked === 1,
    `selected=${opened.selected} marked=${opened.marked}`,
  );
  check(
    "an icon is drawn as an image and a missing one as a letter tile",
    opened.icons === HITS.filter((h) => h.icon).length &&
      opened.letters === HITS.filter((h) => !h.icon).length,
    `icons=${opened.icons} letters=${opened.letters}`,
  );
  check(
    "the page never overflows its window, the list is what scrolls",
    opened.overflowX <= 0 && opened.overflowY <= 0 && opened.listScrolls,
    `x=${opened.overflowX} y=${opened.overflowY} list=${
      opened.listScrolls ? "scrolls" : "does not scroll"
    }`,
  );

  // from here on, anything logged is this probe's own doing
  session.errors.length = 0;

  await press("arrowdown");
  await wait(150);
  const down = await session.evaluate(MEASURE);
  check(
    "ArrowDown selects the row below",
    down.selected === 1 && down.marked === 1 && down.titles[down.selected] === HITS[1].title,
    `selected=${down.selected} marked=${down.marked}`,
  );
  check("the selected row is inside the list", down.inView);

  await press("arrowup");
  await wait(150);
  const up = await session.evaluate(MEASURE);
  check(
    "ArrowUp selects the row above",
    up.selected === 0 && up.marked === 1,
    `selected=${up.selected} marked=${up.marked}`,
  );

  // A whole string at once: one `Input.insertText` is one keystroke as far as
  // the page is concerned, and twelve round trips would be twelve searches.
  await session.send("Input.insertText", { text: "zzz" });
  await wait(400);
  const nothing = await session.evaluate(MEASURE);
  check(
    "a query the backend has nothing for leaves the list empty",
    nothing.titles.length === 0 && nothing.selected === -1,
    `rows=${nothing.titles.length} selected=${nothing.selected}`,
  );
  check(
    "the empty list says so rather than throwing",
    (nothing.empty ?? "").includes("nothing matches"),
    JSON.stringify(nothing.empty),
  );
  check(
    "the backend was asked for what was typed",
    lastSearch(nothing)?.args?.query === "zzz",
    JSON.stringify(lastSearch(nothing)?.args ?? null),
  );
  check("nothing threw while searching", quiet(session).length === 0, quiet(session).join(" | "));
  session.errors.length = 0;

  await press("ctrl+a");
  await session.send("Input.insertText", { text: "arc" });
  await wait(400);
  const narrowed = await session.evaluate(MEASURE);
  check(
    "typing a narrower query narrows the list",
    narrowed.titles.join(" | ") === "Arc",
    `input=${JSON.stringify(narrowed.input)} rows=${narrowed.titles.join(" | ")}`,
  );
  check(
    "the search call carried the query, not an older one",
    lastSearch(narrowed)?.args?.query === "arc",
    JSON.stringify(lastSearch(narrowed)?.args ?? null),
  );
  check(
    "the narrowed list keeps exactly one row selected",
    narrowed.selected === 0 && narrowed.marked === 1,
    `selected=${narrowed.selected} marked=${narrowed.marked}`,
  );

  await press("enter");
  await wait(300);
  const ran = await session.evaluate(MEASURE);
  const run = of(ran, "floaty_palette_run").at(-1);
  check("Enter runs a hit", run !== undefined, "floaty_palette_run was never invoked");
  check(
    "the hit it runs is the selected row",
    run?.args?.hit?.title === "Arc" && run?.args?.hit?.kind === "app",
    JSON.stringify(run?.args?.hit ?? null),
  );
  check(
    "the input is cleared after a run",
    ran.input === "",
    `value=${JSON.stringify(ran.input)}`,
  );

  await press("escape");
  await wait(300);
  const hid = await session.evaluate(MEASURE);
  check(
    "Escape asks the window to hide",
    of(hid, "floaty_palette_hide").length >= 1,
    JSON.stringify(hid.calls.map((c) => c.cmd)),
  );

  if (shiny) await session.screenshot(path.join(here, "out", "palette.png"));
  check("no console errors or exceptions for the whole run", quiet(session).length === 0, quiet(session).join(" | "));
} finally {
  chrome.stop();
}

console.log(failed === 0 ? "\nthe palette behaves" : `\n${failed} palette check(s) need a look`);
process.exit(failed === 0 ? 0 : 1);
