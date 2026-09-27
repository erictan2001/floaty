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
 * The whole world is a fixture of its own (see `runtime.mjs`, the pattern
 * `verify/redo.mjs` uses): the desktop it drags on, the store, the log — all scratch. The
 * floatie it drags is one the probe made, not one of the user's: two throwaway files
 * (`zz-drag-probe-a-*`, `zz-drag-probe-b-*`) are written into the fixture's own root, one
 * is dragged across the boundary and back, and both are removed at the end. Nothing on
 * the user's desktop is ever picked up, moved or dropped: a drop that lands on a folder
 * merges the probe's own file, and before it deletes what it made the probe reverses *its
 * own* steps through the app's own undo (`floaty_undo`, the Ctrl+Alt+Z action) — never
 * the user's, which may sit underneath on the same stack. A desktop this probe ran on is
 * the desktop it started with, and because the content is known that is an exact assertion
 * rather than a hopeful one.
 *
 * Usage: node verify/drag.mjs [--port 9222]
 *   exit 0  the floatie crossed, came back, and was under the pointer at every leg
 *   exit 1  the app is wrong (nowhere, on both screens, unmoved, or misplaced)
 *   exit 2  the probe could not run (the app never came up, one screen, nothing to drag)
 */
import { existsSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import os from "node:os";
import { APP_PORT, Session } from "./cdp.mjs";
import { App, Fixture, flag, launch, overlays, probe, samePath } from "./runtime.mjs";

const port = Number(flag("port", APP_PORT));
const { check, skip, finish, problems } = probe("drag");

/** A failed check, in the order it reads: the assertion, then what explains it. */
const fail = (what, lines = []) => {
  check(false, what);
  for (const line of lines) console.error(`  - ${line}`);
};

const fixture = Fixture.create({ dir: join(os.tmpdir(), `floaty-drag-${process.pid}`) });
const app = await launch({ fixture, cwd: resolve(import.meta.dirname, ".."), port }).catch((error) =>
  skip(error.message),
);
// Registered after the launch, so it runs *after* the launch's own exit hook: on Windows
// the scratch folder cannot be deleted while the app still has it open. It is also what
// makes every `skip` below safe — the app dies with the process, so nothing is left drawn
// on the user's screen and nothing is left holding the port.
process.on("exit", () => {
  try {
    fixture.remove();
  } catch {
    /* the app may still have it open, and a scratch folder is the temp cleaner's problem */
  }
});

/** The throwaway files this probe wrote, so cleanup can take them back. */
const made = [];
/**
 * The undo depth before the probe's own drags. Cleanup undoes down to here and no
 * further: a step that was not this probe's to push is not this probe's to pop, and in a
 * world of the probe's own that is the only thing that separates the two.
 */
let undoFloor = null;

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

// Two screens, or there is nothing to test. The app counts itself up as soon as the first
// overlay answers, a moment before it has made the window for the second screen, so wait
// for that window: counting once and calling a two-screen desktop a one-screen one is a
// false answer, and a false answer here is how the bug this probe exists for went unseen.
if (app.sessions.length < 2) {
  await app
    .until("an overlay window for a second screen", async () =>
      (await overlays(app.port)).length >= 2 ? true : null,
    )
    .catch(() => undefined);
  if (app.sessions.length < 2) {
    const found = app.sessions.length;
    await skip(
      `needs two desktop overlays on port ${port}, found ${found} — this desktop ` +
        `has ${found === 1 ? "one screen" : `no running app on ${port}`}`,
    );
  }
  // The one thing the runtime cannot do for itself: a session for a window the app made
  // after it counted the pages. Pushed onto the app's own list, so the indices `drawnBy`
  // hands back keep naming the screens the survey found them on.
  app.sessions.push(await Session.open(app.port, "#/overlay", 1));
}

// The records the probe found the desktop with, so "it left the world as it found it" is a
// comparison and not a claim.
const beforeIds = (await app.list()).map((r) => r.id).sort();

// The two artifacts this probe owns, in the fixture's own root. The stamp keeps two
// concurrent runs from fighting over the same name; the app picks both up as floaties the
// moment they exist — which also exercises the watcher on a file it did not know about.
const stamp = String(process.pid);
const fileA = fixture.inRoot(`zz-drag-probe-a-${stamp}.txt`);
const fileB = fixture.inRoot(`zz-drag-probe-b-${stamp}.txt`);
writeFileSync(fileA, "floaty drag probe\n");
made.push(fileA);
writeFileSync(fileB, "floaty drag probe\n");
made.push(fileB);

/** Wait for a record whose path is `target` — or for one to go away. */
const byPath = (target, want = true) =>
  app.until(
    `${want ? "a record for" : "no record for"} ${target}`,
    async () => {
      const records = await app.list();
      const paths = await Promise.all(records.map((r) => app.pathOf(r)));
      const found = records.find((r, i) => samePath(paths[i], target));
      if (want ? !found : Boolean(found)) return null;
      return found ?? true;
    },
    // The app's watcher debounces a new file for a few seconds before it syncs; a short
    // deadline here fails the probe on the app being careful.
    { timeout: 60 },
  );

// A file that never became a floatie is the watcher not picking it up, not the app being
// wrong — the same two answers the old fixed 60x250ms poll gave, without the 15s guess.
const recA = await byPath(fileA).catch(() => null);
const recB = await byPath(fileB).catch(() => null);
if (recA === true || !recA || recB === true || !recB) {
  await skip("the probe's two throwaway files did not become floaties");
}

// Which window is the one to drag *from*: ask them, rather than trusting an order.
const surveyed = [];
for (let nth = 0; nth < app.sessions.length; nth += 1) {
  surveyed.push({ nth, ...(await app.page(nth).evaluate(SURVEY)) });
}
const id = recA.id;
const source = surveyed.find((s) => s.ids.includes(id));
if (!source) await skip(`the probe's own floatie ${id} is not mounted by any overlay window`);
const other = surveyed.find((s) => s.screen !== source.screen);
if (!other) await skip(`both overlays report the same screen (${source.screen})`);

const recordNow = async () => (await app.list()).find((x) => x.id === id) ?? null;
const beforeRec = await recordNow();
if (!beforeRec) await skip(`no record ${id}`);
const before = [beforeRec.x, beforeRec.y];
// A merge is a real file move, so the undo stack is how this probe can tell that
// dragging a floatie onto another screen did *not* quietly fold it into a folder. The
// *label* matters, not the depth: a drag that merely moves something is undoable too, so a
// deeper stack on its own means nothing (reading it as a merge is a false alarm this
// probe has already raised once). The depth is remembered so cleanup can undo exactly the
// steps this probe adds and no further.
const undoStateBefore = await app.tryInvoke("floaty_undo_state");
const undoBefore = undoStateBefore?.label ?? null;
undoFloor = undoStateBefore?.depth ?? null;

// Grab the floatie in its middle and pull `past` css px beyond this window's right edge —
// which on a two-screen desktop is a place on the next screen, *not* in the band.
const box = await app.page(source.nth).evaluate(`(() => {
  const el = document.getElementById("slot-" + ${JSON.stringify(id)});
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) };
})()`);
if (!box) await skip(`floatie ${id} is not mounted by the window on ${source.screen}`);

const aim = await app.page(source.nth).evaluate(AIM(source.screen, other.screen, id));
if (aim.error) await skip(aim.error);
if (aim.client.x >= 0 && aim.client.x <= source.width) {
  await skip(
    `the middle of ${other.screen} is inside ${source.screen}'s own window (client x ` +
      `${aim.client.x} of ${source.width}) — the screens overlap, so there is nothing to test`,
  );
}

/**
 * A place that has stopped changing, or has arrived.
 *
 * These three waits are measurements, not milestones: waiting for the floatie to *be*
 * where the pointer put it would make the check that follows say nothing, and a fixed
 * delay is a guess at how fast this machine renders. So wait for one of the two signs
 * that the render is done — it reached the place, or two reads a poll apart agree — and
 * hand back the resting value whatever it turned out to be. A window that never mounts the
 * slot at all answers nothing forever; that is the `missing` leg, not a timeout.
 */
const rested = (what, read, near = null) => {
  let previous = null;
  return app.until(what, async () => {
    const now = await read();
    if (!now) return null;
    if (near && Math.hypot(now.x - near.x, now.y - near.y) <= 2) return now;
    const still = previous && now.x === previous.x && now.y === previous.y;
    previous = now;
    return still ? now : null;
  });
};

/** Where the record sits and which screen that is, by the app's own rectangles. */
const where = async () => {
  const r = await recordNow();
  if (!r) return null;
  return { x: r.x, y: r.y, xy: [r.x, r.y], screen: await app.screenOf(r) };
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
const whereDrawn = async (nth, expect, label) => {
  const at = await rested(
    `${id} to come to rest under the pointer (${label})`,
    () => app.page(nth).evaluate(SLOT_AT(id)),
    expect,
  ).catch(() => null);
  const state = await app.page(nth).evaluate(STATE(id)).catch(() => null);
  if (!at) return { label, missing: true, state };
  return { label, off: Math.hypot(at.x - expect.x, at.y - expect.y), at, expect, state };
};

// The drag goes to the window the pointer starts in, and `mouse`/`moveTo` send to page 0.
// A one-page App over the source window is the same code and the same transport, aimed at
// the right window.
const hand = new App(app.port, [app.page(source.nth)]);

await hand.mouse("mousePressed", box.x, box.y);
// Straight there, in steps, so the page sees a real drag rather than one jump.
await hand.moveTo(aim.across.client.x, aim.across.client.y, { from: box });
const legs = [];
legs.push(await whereDrawn(other.nth, aim.across.expect, `just onto ${other.screen}`));
await hand.moveTo(aim.client.x, aim.client.y, { from: aim.across.client });
legs.push(await whereDrawn(other.nth, aim.expect, `out on ${other.screen}`));
await hand.moveTo(aim.back.client.x, aim.back.client.y, { from: aim.client });
legs.push(await whereDrawn(source.nth, aim.back.expect, `back on ${source.screen}`));
await hand.mouse("mouseReleased", aim.back.client.x, aim.back.client.y);
// The drop settles: the record follows the pointer to the end of the drag and then stops.
const after = await rested(`${id} to come to rest where the pointer left it`, where, aim.back.expect);

const drawn = (await app.drawnBy(id)).map((nth) => surveyed[nth]?.screen ?? `page ${nth}`);

for (const leg of legs) {
  if (leg.missing) {
    fail(
      `mid-drag (${leg.label}), no window was drawing ${id} — it is following neither the ` +
        "pointer nor the screen it is on",
    );
    continue;
  }
  if (leg.off > 6) {
    fail(
      `mid-drag (${leg.label}), ${id} was drawn ${leg.off.toFixed(0)}px from the pointer: at ` +
        `${leg.at.x},${leg.at.y} while the pointer held it at ${leg.expect.x},${leg.expect.y}`,
      [`window: ${JSON.stringify(leg.state)}`],
    );
  }
}
if (!after) fail(`record ${id} is gone from the store`);
else {
  if (after.screen !== source.screen) {
    fail(
      `record ${id} is on ${after.screen} after a drag that crossed to ${aim.destScreen} and ` +
        `came back — it should be on ${source.screen}, under the pointer`,
    );
  }
  const off = Math.hypot(after.xy[0] - aim.back.expect.x, after.xy[1] - aim.back.expect.y);
  if (off > 4) {
    fail(
      `record ${id} ended at ${after.xy} but the pointer brought it back to ` +
        `${aim.back.expect.x},${aim.back.expect.y} — the place it is given is not the pointer's own`,
    );
  }
  if (!after.screen) fail(`record ${id} is at ${after.xy} — on no screen at all (the band)`);
}
if (drawn.length === 0) fail(`no window draws ${id} — invisible and unclickable`);
if (drawn.length > 1) fail(`two windows draw ${id}: ${drawn.join(", ")}`);
if (drawn.length === 1 && drawn[0] !== after?.screen) {
  fail(`drawn by the window on ${drawn[0]} but stored as being on ${after.screen}`);
}
// The second throwaway is the probe's own neighbour: if the drag swallowed anything, it
// swallowed this, not a folder the user owns. It has to still be its own record, with its
// file still on disk.
const recBAfter = (await app.list()).find((r) => r.id === recB.id);
if (!recBAfter) fail(`the probe's second throwaway ${recB.id} was swallowed by the drag`);
else {
  const movedTo = await app.pathOf(recBAfter);
  if (!samePath(movedTo, fileB)) fail(`the probe's second throwaway's file moved to ${movedTo}`);
}
if (!existsSync(fileB)) fail(`the probe's second throwaway's file is gone from disk`);
const undoAfter = (await app.tryInvoke("floaty_undo_state"))?.label ?? null;
if (undoAfter === "merge" && undoBefore !== "merge") {
  fail(
    "the drag ran a merge — a real file was moved by a drag that only meant to put the " +
      "floatie on the other screen",
  );
}
const errors = app.takeErrors();
if (errors.length) fail(`page errors: ${errors.slice(0, 2).join(" | ")}`);
const expected =
  `aimed at physical ${aim.physical.join(",")} = the middle of ${other.screen}, ` +
  `expecting record space ${aim.expect.x},${aim.expect.y}`;

/**
 * Take back everything this probe made: first its own undo steps (which reverses a merge
 * the drop caused, putting the probe's file back out of whatever it folded into), then the
 * two throwaway files, then wait for their floaties to go.
 */
const clean = async () => {
  if (undoFloor !== null) {
    await app
      .until(`${id} to be off the undo stack`, async () => {
        const state = await app.tryInvoke("floaty_undo_state");
        if (!state || state.depth <= undoFloor) return true;
        await app.tryInvoke("floaty_undo");
        return null;
      })
      .catch(() => undefined);
  }
  for (const file of made) rmSync(file, { force: true });
  await app
    .until("the probe's own floaties to go", async () => {
      const records = await app.list().catch(() => null);
      if (!records) return true;
      const paths = await Promise.all(records.map((r) => app.pathOf(r)));
      return paths.some((p) => made.some((file) => samePath(p, file))) ? null : true;
    })
    .catch(() => undefined);
};

if (!problems.length) {
  // Put it back: the probe moves a floatie it made, and the way home is the same handover.
  await app.invoke("floaty_drag_to", { id, x: before[0], y: before[1] });
  const back = await rested(`${id} to come to rest where it was`, where).catch(() => null);
  if (!back || back.xy[0] !== before[0] || back.xy[1] !== before[1]) {
    fail(`dragging ${id} home did not work`, [`asked for ${before}, got ${JSON.stringify(back)}`]);
  }

  // Take the probe's own steps off the undo stack (reversing a merge if the drop made one),
  // delete the two files it wrote, and prove the desktop is back to what it was: the same
  // records, none of the probe's files left behind.
  await clean();
  const afterIds = (await app.list()).map((r) => r.id).sort();
  const leaked = afterIds.filter((x) => !beforeIds.includes(x));
  const missing = beforeIds.filter((x) => !afterIds.includes(x));
  const stillOnDisk = made.filter((file) => existsSync(file));
  if (leaked.length || missing.length || stillOnDisk.length) {
    fail("the probe left the desktop changed", [
      `records added: ${JSON.stringify(leaked)}`,
      `records gone: ${JSON.stringify(missing)}`,
      `probe files still on disk: ${JSON.stringify(stillOnDisk)}`,
    ]);
  }
}

if (!problems.length) {
  console.log(
    `drag: the probe's own ${id} crossed ${source.screen} → ${other.screen} → ${source.screen} ` +
      `(${before.join(",")} → ${after.xy.join(",")}; ${expected}), under the pointer at every ` +
      `leg (${legs.map((l) => `${l.label}: ${l.off === undefined ? "not drawn" : `${l.off.toFixed(0)}px`}`).join(", ")}) ` +
      `— ${beforeIds.length} user records before and after, 0 probe files left`,
  );
} else {
  // Printed last, so it covers every failure including the two above it — a header that
  // arrives before them describes only part of what went wrong.
  console.error(
    `drag: FAILED — dragged the probe's own ${id} from ${source.screen} to ${other.screen} and back (${expected})`,
  );
  console.error(`  - measured: ${JSON.stringify(after)} drawn by ${drawn.join(", ") || "nobody"}`);
  console.error(
    `  - the pointer left it at ${aim.back.expect.x},${aim.back.expect.y} on ${source.screen}`,
  );
}
await finish(clean, () => app.stop(), () => fixture.remove());
