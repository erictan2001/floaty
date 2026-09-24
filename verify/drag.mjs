/**
 * Drag a floatie across a screen boundary and check it arrives — whole, on the other
 * screen, drawn by the window whose screen it is now on.
 *
 * This is the probe for a bug no other probe could see: the drag used to place the
 * floatie from the *deltas* of a pointer in the window the drag began in, so a drag from
 * the primary onto the second screen landed in the band between the two — 1440 + px past
 * the edge, while the second screen's own space starts at 1920 on a 2x/1.5x pair. No
 * window owns the band, so the floatie was drawn by nobody: invisible, unclickable, and
 * still in the store. It only exists on two screens at different scales, which is why it
 * is its own probe and why it skips (exit 2) on a single-screen desktop.
 *
 * The floatie it drags is one the probe made, not one of the user's. Two throwaway files
 * (`zz-drag-probe-a-*`, `zz-drag-probe-b-*`) are written into the desktop root, one is
 * dragged across the boundary and back, and both are removed at the end — the pattern
 * `verify/redo.mjs` uses. Nothing on the user's desktop is ever picked up, moved or
 * dropped: a drop that lands on a folder merges the probe's own file, and before it
 * deletes what it made the probe reverses *its own* steps through the app's own undo
 * (`floaty_undo`, the Ctrl+Alt+Z action) — never the user's, which may sit underneath on
 * the same stack. A desktop this probe ran on is the desktop it started with.
 *
 * Usage: node verify/drag.mjs [--port 9222]
 *   exit 0  the floatie crossed, came back, and was under the pointer at every leg
 *   exit 1  the app is wrong (nowhere, on both screens, unmoved, or misplaced)
 *   exit 2  the probe could not run (no app on the port, one screen, nothing to drag)
 */
import { writeFileSync, rmSync, existsSync } from "node:fs";
import { join } from "node:path";
import { APP_PORT, Session, listTargets, realErrors } from "./cdp.mjs";

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};
const port = Number(flag("port", APP_PORT));

const sessions = [];
const surveyed = [];
/** The throwaway files this probe wrote, so cleanup can take them back. */
const made = [];
const madePaths = new Set();
/**
 * The undo depth before the probe's own drags. Cleanup undoes down to here and no
 * further: the user's own work can sit underneath on the same stack, and emptying the
 * stack — the way a probe that did not think about it would — would take that back too.
 */
let undoFloor = null;

const list = () => sessions[0].invoke("floaty_list");
const pathOf = (r) => r.data?.target ?? r.data?.path ?? "";

/**
 * Take back everything this probe made: first its own undo steps (which reverses a merge
 * the drop caused, putting the probe's file back out of whatever it folded into), then the
 * two throwaway files, then wait for their floaties to go.
 */
const cleanup = async () => {
  const session = sessions[0];
  if (!session) return;
  if (undoFloor !== null) {
    for (let i = 0; i < 24; i += 1) {
      const state = await session.invoke("floaty_undo_state").catch(() => null);
      if (!state || state.depth <= undoFloor) break;
      await session.invoke("floaty_undo").catch(() => undefined);
      await new Promise((r) => setTimeout(r, 250));
    }
  }
  for (const path of made) rmSync(path, { force: true });
  for (let i = 0; i < 40; i += 1) {
    const records = await session.invoke("floaty_list").catch(() => null);
    if (!records) break;
    if (!records.some((r) => madePaths.has(pathOf(r).toLowerCase()))) break;
    await new Promise((r) => setTimeout(r, 250));
  }
};

/** Clean up, say why, close the sockets, exit. */
const finish = async (code, lines = []) => {
  await cleanup().catch(() => undefined);
  for (const line of lines) console.error(line);
  for (const s of sessions) await s.close().catch(() => undefined);
  process.exit(code);
};
const skip = (message) => finish(2, [`drag: ${message}`]);
const failed = (message, detail = []) =>
  finish(1, [`drag: FAILED — ${message}`, ...detail.map((line) => `  - ${line}`)]);

/** What one overlay window knows: its screen, and the floaties that live on it. */
const SURVEY = `
const invoke = window.__TAURI_INTERNALS__.invoke;
const area = await invoke("floaty_overlay_area");
const screen = area ? area.screen : null;
const records = await invoke("floaty_list");
const mine = screen ? records.filter((r) =>
  r.x >= screen.logical.x && r.x < screen.logical.x + screen.logical.w &&
  r.y >= screen.logical.y && r.y < screen.logical.y + screen.logical.h) : [];
return { screen: screen ? screen.name : null, width: screen ? Math.round(screen.logical.w) : 0,
  mounted: document.querySelectorAll(".overlay-slot").length,
  ids: mine.map((r) => r.id), first: mine[0] ? mine[0].id : null };
`;

/**
 * Aim at the middle of another screen, and say what that means on both sides of the
 * mapping: the css px to send the pointer to in *this* window, and the record-space
 * place the floatie should end up in.
 *
 * The floatie's own y is not usable as a target — a short second screen has no such
 * place, and the pointer would be aimed below it — so the destination is a point that
 * exists on the target screen by construction.
 */
const AIM = (ownScreen, otherScreen, recordId) => `
const invoke = window.__TAURI_INTERNALS__.invoke;
const area = await invoke("floaty_overlay_area");
const list = await invoke("floaty_screens");
const src = list.find((s) => s.name === ${JSON.stringify(ownScreen)});
const dst = list.find((s) => s.name === ${JSON.stringify(otherScreen)});
if (!src || !dst) return { error: "a screen went away mid-probe" };
const rec = (await invoke("floaty_list")).find((r) => r.id === ${JSON.stringify(recordId)});
if (!rec) return { error: "the floatie went away mid-probe" };
const el = document.getElementById("slot-" + ${JSON.stringify(recordId)});
if (!el) return { error: "the floatie is not mounted" };
const box = el.getBoundingClientRect();
const grab = { x: Math.round(box.x + box.width / 2), y: Math.round(box.y + box.height / 2) };
// the record-space point the pointer holds on the floatie
const scale = window.devicePixelRatio || src.scale;
const held = {
  x: src.logical.x + ((src.physical.x + grab.x * scale - src.physical.x) / src.scale) - rec.x,
  y: src.logical.y + ((src.physical.y + grab.y * scale - src.physical.y) / src.scale) - rec.y,
};
// the middle of the other screen, in that window's own css px
const dst_local = { x: dst.logical.w / 2, y: dst.logical.h / 2 };
const dst_virtual = { x: dst.logical.x + dst_local.x, y: dst.logical.y + dst_local.y };
// ... expressed in this window's css px: same physical point, back through this scale
const dst_physical = {
  x: dst.physical.x + dst_local.x * dst.scale,
  y: dst.physical.y + dst_local.y * dst.scale,
};
// Home is the floatie's own place, and only if nothing else is there: a desktop is full of
// folders, and dropping a file onto one merges it — the app doing its job, and a probe
// quietly moving a real file. If that place is taken, the nearest free spot on this screen
// is used instead, which is just as good a claim to test.
const others = (await invoke("floaty_list")).filter((r) =>
  r.id !== ${JSON.stringify(recordId)} && ["file", "app", "folder"].includes(r.kind));
const size = { w: rec.w ?? 92, h: rec.h ?? 112 };
// Exactly the app's own merge rule (the dropped tile's *centre* against each other
// item's rect inflated by 12px), so a spot this accepts is a spot the drop will not
// merge — a stricter test of my own would move floaties the app would have left alone.
const free = (x, y) => {
  const cx = x + size.w / 2;
  const cy = y + size.h / 2;
  return !others.some((o) => {
    const ow = o.w ?? 92;
    const oh = o.h ?? 112;
    return cx >= o.x - 12 && cx < o.x + ow + 12 && cy >= o.y - 12 && cy < o.y + oh + 12;
  });
};
const spots = [[rec.x, rec.y]];
for (let dy = 0; dy <= 600 && spots.length < 60; dy += 120) {
  for (let dx = 0; dx <= 900; dx += 120) {
    spots.push([rec.x + dx, rec.y + dy], [rec.x - dx, rec.y + dy]);
  }
}
const home = spots.find(([x, y]) =>
  x >= src.logical.x + 8 && y >= src.logical.y + 8 &&
  x + size.w <= src.logical.x + src.logical.w - 8 && y + size.h <= src.logical.y + src.logical.h - 8 &&
  free(x, y));
if (!home) return { error: "no free place on " + src.name + " to drop the floatie back on" };
const back = {
  client: {
    x: Math.round(home[0] + size.w / 2 - src.logical.x),
    y: Math.round(home[1] + size.h / 2 - src.logical.y),
  },
  expect: { x: home[0], y: home[1] },
};
return {
  box: grab,
  client: {
    x: Math.round((dst_physical.x - src.physical.x) / scale),
    y: Math.round((dst_physical.y - src.physical.y) / scale),
  },
  expect: { x: Math.round(dst_virtual.x - held.x), y: Math.round(dst_virtual.y - held.y) },
  destScreen: dst.name,
  physical: [Math.round(dst_physical.x), Math.round(dst_physical.y)],
  // ...and a place just inside the other screen, for checking mid-drag that the window
  // drawing the floatie is actually following the pointer, not just receiving it once
  back,
  across: (() => {
    const local = { x: 60, y: dst.logical.h / 2 };
    const physical = { x: dst.physical.x + local.x * dst.scale, y: dst.physical.y + local.y * dst.scale };
    return {
      client: { x: Math.round((physical.x - src.physical.x) / scale), y: Math.round((physical.y - src.physical.y) / scale) },
      expect: { x: Math.round(dst.logical.x + local.x - held.x), y: Math.round(dst.logical.y + local.y - held.y) },
    };
  })(),
};
`;

/**
 * Everything a leg's failure needs: which screen this window thinks it is, its scale,
 * the screens it can map through, where the record is, and where (if anywhere) it is
 * drawing the floatie — in record space.
 */
const STATE = (id) => `
const invoke = window.__TAURI_INTERNALS__.invoke;
const area = await invoke("floaty_overlay_area");
const list = await invoke("floaty_screens");
const rec = (await invoke("floaty_list")).find((r) => r.id === ${JSON.stringify(id)});
const el = document.getElementById("slot-" + ${JSON.stringify(id)});
let slot = null;
if (el && area) {
  const m = new DOMMatrixReadOnly(getComputedStyle(el).transform);
  slot = [Math.round(area.screen.logical.x + m.m41), Math.round(area.screen.logical.y + m.m42)];
}
return {
  screen: area ? area.screen.name : null,
  dpr: window.devicePixelRatio,
  inner: [window.innerWidth, window.innerHeight],
  screens: list.map((s) => s.name + "=" + s.physical.x + "," + s.physical.y + " " + s.physical.w + "x" +
    s.physical.h + "@" + s.scale + "|logical " + s.logical.x + "," + s.logical.y + " " + s.logical.w + "x" + s.logical.h),
  record: rec ? [rec.x, rec.y] : null,
  slot,
};
`;

/** Where the window that should be drawing it says the floatie is (its own css px). */
const SLOT_AT = (id) => `
const el = document.getElementById("slot-" + ${JSON.stringify(id)});
if (!el) return null;
const m = new DOMMatrixReadOnly(getComputedStyle(el).transform);
const area = await window.__TAURI_INTERNALS__.invoke("floaty_overlay_area");
return { x: Math.round(area.screen.logical.x + m.m41), y: Math.round(area.screen.logical.y + m.m42) };
`;

const targets = await listTargets(port).catch(() => []);
const overlays = targets.filter((t) => t.type === "page" && t.url.endsWith("#/overlay"));
if (overlays.length < 2) {
  await skip(
    `needs two desktop overlays on port ${port}, found ${overlays.length} — this desktop ` +
      `has ${overlays.length === 1 ? "one screen" : `no running app on ${port}`}`,
  );
}

for (let nth = 0; nth < overlays.length; nth += 1) {
  sessions.push(await Session.open(port, "#/overlay", nth));
}

// The desktop root, from a record that is already in it — the same read redo.mjs does.
// A file floatie keeps its path under `target`; folders use `path`.
const recordsBefore = await list();
const beforeIds = recordsBefore.map((r) => r.id).sort();
const anyFile = recordsBefore.find((r) => r.kind !== "folder" && pathOf(r).includes("\\"));
if (!anyFile) await skip("no file floatie to read the desktop root from");
const root = pathOf(anyFile).slice(0, pathOf(anyFile).lastIndexOf("\\"));

// The two artifacts this probe owns. The stamp keeps two concurrent runs from fighting
// over the same name; the app picks both up as floaties the moment they exist.
const stamp = String(process.pid);
const fileA = join(root, `zz-drag-probe-a-${stamp}.txt`);
const fileB = join(root, `zz-drag-probe-b-${stamp}.txt`);
writeFileSync(fileA, "floaty drag probe\n");
made.push(fileA);
madePaths.add(fileA.toLowerCase());
writeFileSync(fileB, "floaty drag probe\n");
made.push(fileB);
madePaths.add(fileB.toLowerCase());

/** Wait for a record whose target is `path` (or for it to go away). */
const byPath = async (path, want = true, tries = 60) => {
  for (let i = 0; i < tries; i += 1) {
    const found = (await list()).find((r) => pathOf(r).toLowerCase() === path.toLowerCase());
    if (Boolean(found) === want) return found ?? true;
    await new Promise((r) => setTimeout(r, 250));
  }
  return null;
};

const recA = await byPath(fileA, true);
const recB = await byPath(fileB, true);
if (recA === true || !recA || recB === true || !recB) {
  await skip("the probe's two throwaway files did not become floaties");
}

// Which window is the one to drag *from*: ask them, rather than trusting an order.
for (let nth = 0; nth < sessions.length; nth += 1) {
  const survey = await sessions[nth].evaluate(SURVEY);
  surveyed.push({ nth, session: sessions[nth], ...survey });
}
const id = recA.id;
const source = surveyed.find((s) => s.ids.includes(id));
if (!source) await skip(`the probe's own floatie ${id} is not mounted by any overlay window`);
const other = surveyed.find((s) => s.screen !== source.screen);
if (!other) await skip(`both overlays report the same screen (${source.screen})`);

const before = await source.session.evaluate(
  `const l = await window.__TAURI_INTERNALS__.invoke("floaty_list");
   const r = l.find((x) => x.id === ${JSON.stringify(id)}); return r ? [r.x, r.y] : null;`,
);
// A merge is a real file move, so the undo stack is how this probe can tell that
// dragging a floatie onto another screen did *not* quietly fold it into a folder. The
// *label* matters, not the depth: a drag that merely moves something is undoable too, so a
// deeper stack on its own means nothing (reading it as a merge is a false alarm this
// probe has already raised once). The depth is remembered so cleanup can undo exactly the
// steps this probe adds and no further.
const undoStateBefore = await source.session.evaluate(
  `const s = await window.__TAURI_INTERNALS__.invoke("floaty_undo_state");
   return s ? { label: s.label, depth: s.depth } : null;`,
);
const undoBefore = undoStateBefore ? undoStateBefore.label : null;
undoFloor = undoStateBefore ? undoStateBefore.depth : null;
if (!before) await skip(`no record ${id}`);

// Grab the floatie in its middle and pull `past` css px beyond this window's right edge —
// which on a two-screen desktop is a place on the next screen, *not* in the band.
const box = await source.session.evaluate(`(() => {
  const el = document.getElementById("slot-" + ${JSON.stringify(id)});
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) };
})()`);
if (!box) await skip(`floatie ${id} is not mounted by the window on ${source.screen}`);

const aim = await source.session.evaluate(AIM(source.screen, other.screen, id));
if (aim.error) await skip(aim.error);
if (aim.client.x >= 0 && aim.client.x <= source.width) {
  await skip(
    `the middle of ${other.screen} is inside ${source.screen}'s own window (client x ` +
      `${aim.client.x} of ${source.width}) — the screens overlap, so there is nothing to test`,
  );
}

const send = (type, x, y) =>
  source.session.send("Input.dispatchMouseEvent", {
    type,
    x: Math.round(x),
    y: Math.round(y),
    button: "left",
    buttons: type === "mouseReleased" ? 0 : 1,
    clickCount: type === "mouseReleased" ? 1 : 0,
  });

// Straight there, in steps, so the page sees a real drag rather than one jump.
const step_to = async (client, from) => {
  for (let step = 1; step <= 8; step += 1) {
    await send("mouseMoved", from.x + ((client.x - from.x) * step) / 8, from.y + ((client.y - from.y) * step) / 8);
  }
  return client;
};

/**
 * Where a window says it is drawing the floatie, and how far that is from where the
 * pointer is holding it — while the button is still down.
 *
 * Three places are checked this way, because each was a separate bug: just across the
 * boundary (the handover used to tell the other window once, so the floatie stood
 * still), out on the far screen (same), and *back home* — dragging back and forth used
 * to strand the floatie on the other screen, drawn by the wrong window at the
 * coordinates of a drag that had left.
 */
const whereDrawn = async (session, expect, label) => {
  await new Promise((r) => setTimeout(r, 400));
  const at = await session.evaluate(SLOT_AT(id));
  const state = await session.evaluate(STATE(id));
  if (!at) return { label, missing: true, state };
  return { label, off: Math.hypot(at.x - expect.x, at.y - expect.y), at, expect, state };
};

await send("mousePressed", box.x, box.y);
await step_to(aim.across.client, box);
const legs = [];
legs.push(await whereDrawn(other.session, aim.across.expect, `just onto ${other.screen}`));
await step_to(aim.client, aim.across.client);
legs.push(await whereDrawn(other.session, aim.expect, `out on ${other.screen}`));
await step_to(aim.back.client, aim.client);
legs.push(await whereDrawn(source.session, aim.back.expect, `back on ${source.screen}`));
await send("mouseReleased", aim.back.client.x, aim.back.client.y);
await new Promise((r) => setTimeout(r, 600));

const where = async () =>
  source.session.evaluate(`
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const list = await invoke("floaty_screens");
    const r = (await invoke("floaty_list")).find((x) => x.id === ${JSON.stringify(id)});
    if (!r) return null;
    const on = list.find((s) => r.x >= s.logical.x && r.x < s.logical.x + s.logical.w && r.y >= s.logical.y && r.y < s.logical.y + s.logical.h);
    return { xy: [r.x, r.y], screen: on ? on.name : null };
  `);

const after = await where();
const drawn = [];
for (const s of surveyed) {
  const here = await s.session.evaluate(
    `return document.getElementById("slot-" + ${JSON.stringify(id)}) !== null;`,
  );
  if (here) drawn.push(s.screen);
}

const problems = [];
for (const leg of legs) {
  if (leg.missing) {
    problems.push(
      `mid-drag (${leg.label}), no window was drawing ${id} — it is following neither the ` +
        "pointer nor the screen it is on",
    );
    continue;
  }
  if (leg.off > 6) {
    problems.push(
      `mid-drag (${leg.label}), ${id} was drawn ${leg.off.toFixed(0)}px from the pointer: at ` +
        `${leg.at.x},${leg.at.y} while the pointer held it at ${leg.expect.x},${leg.expect.y}` +
        `\n    window: ${JSON.stringify(leg.state)}`,
    );
  }
}
if (!after) problems.push(`record ${id} is gone from the store`);
else {
  if (after.screen !== source.screen) {
    problems.push(
      `record ${id} is on ${after.screen} after a drag that crossed to ${aim.destScreen} and ` +
        `came back — it should be on ${source.screen}, under the pointer`,
    );
  }
  const off = Math.hypot(after.xy[0] - aim.back.expect.x, after.xy[1] - aim.back.expect.y);
  if (off > 4) {
    problems.push(
      `record ${id} ended at ${after.xy} but the pointer brought it back to ` +
        `${aim.back.expect.x},${aim.back.expect.y} — the place it is given is not the pointer's own`,
    );
  }
  if (!after.screen) problems.push(`record ${id} is at ${after.xy} — on no screen at all (the band)`);
}
if (drawn.length === 0) problems.push(`no window draws ${id} — invisible and unclickable`);
if (drawn.length > 1) problems.push(`two windows draw ${id}: ${drawn.join(", ")}`);
if (drawn.length === 1 && drawn[0] !== after?.screen) {
  problems.push(`drawn by the window on ${drawn[0]} but stored as being on ${after.screen}`);
}
// The second throwaway is the probe's own neighbour: if the drag swallowed anything, it
// swallowed this, not a folder the user owns. It has to still be its own record, with its
// file still on disk.
const recBAfter = (await list()).find((r) => r.id === recB.id);
if (!recBAfter) problems.push(`the probe's second throwaway ${recB.id} was swallowed by the drag`);
else if (pathOf(recBAfter).toLowerCase() !== fileB.toLowerCase()) {
  problems.push(`the probe's second throwaway's file moved to ${pathOf(recBAfter)}`);
}
if (!existsSync(fileB)) problems.push(`the probe's second throwaway's file is gone from disk`);
const undoAfter = await source.session.evaluate(
  `const s = await window.__TAURI_INTERNALS__.invoke("floaty_undo_state"); return s ? s.label : null;`,
);
if (undoAfter === "merge" && undoBefore !== "merge") {
  problems.push(
    "the drag ran a merge — a real file was moved by a drag that only meant to put the " +
      "floatie on the other screen",
  );
}
const errors = realErrors(source.session.errors);
if (errors.length) problems.push(`page errors: ${errors.slice(0, 2).join(" | ")}`);
const expected =
  `aimed at physical ${aim.physical.join(",")} = the middle of ${other.screen}, ` +
  `expecting record space ${aim.expect.x},${aim.expect.y}`;
if (problems.length) {
  await failed(
    `dragged the probe's own ${id} from ${source.screen} to ${other.screen} and back (${expected})`,
    [
      ...problems,
      `measured: ${JSON.stringify(after)} drawn by ${drawn.join(", ") || "nobody"}`,
      `the pointer left it at ${aim.back.expect.x},${aim.back.expect.y} on ${source.screen}`,
    ],
  );
}

// Put it back: the probe moves a floatie it made, and the way home is the same handover.
await source.session.evaluate(
  `await window.__TAURI_INTERNALS__.invoke("floaty_drag_to", { id: ${JSON.stringify(id)}, x: ${before[0]}, y: ${before[1]} });`,
);
await new Promise((r) => setTimeout(r, 400));
const back = await where();
if (!back || back.xy[0] !== before[0] || back.xy[1] !== before[1]) {
  await failed(`dragging ${id} home did not work`, [`asked for ${before}, got ${JSON.stringify(back)}`]);
}

// Take the probe's own steps off the undo stack (reversing a merge if the drop made one),
// delete the two files it wrote, and prove the desktop is back to what it was: the same
// records, none of the probe's files left behind.
await cleanup();
const afterIds = (await list()).map((r) => r.id).sort();
const leaked = afterIds.filter((x) => !beforeIds.includes(x));
const missing = beforeIds.filter((x) => !afterIds.includes(x));
const stillOnDisk = made.filter((path) => existsSync(path));
if (leaked.length || missing.length || stillOnDisk.length) {
  await failed("the probe left the desktop changed", [
    `records added: ${JSON.stringify(leaked)}`,
    `records gone: ${JSON.stringify(missing)}`,
    `probe files still on disk: ${JSON.stringify(stillOnDisk)}`,
  ]);
}

console.log(
  `drag: the probe's own ${id} crossed ${source.screen} → ${other.screen} → ${source.screen} ` +
    `(${before.join(",")} → ${after.xy.join(",")}; ${expected}), under the pointer at every ` +
    `leg (${legs.map((l) => `${l.label}: ${l.off === undefined ? "not drawn" : `${l.off.toFixed(0)}px`}`).join(", ")}) ` +
    `— ${beforeIds.length} user records before and after, 0 probe files left`,
);
await finish(0);
