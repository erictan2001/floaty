/**
 * A widget dropped where nothing draws it has to come back, the way a tile does.
 *
 * The drag itself is free to leave the desktop, and it must be: a drag that crosses from one
 * screen to another passes through the band between them, which belongs to no screen, and the
 * record has to follow the pointer through it. What must not happen is the *drop* leaving it
 * there — no overlay window covers that band, so nothing draws the widget and nothing can be
 * clicked to bring it back. It is gone until floaty is restarted.
 *
 * Tiles re-home at the drop (`floaty_dropped_inner` -> `rehome_stranded`). Panels release
 * through `floaty_gesture_end` instead, which never did; the fix is there, and this is the
 * check. The two screens here are 2880x1920@2 and 1920x1080@1.5, so their logical rectangles
 * do not touch: there is a 480px band belonging to no screen at all.
 *
 * The release is driven with the same two commands the page's own release sends —
 * `floaty_drag_to` for the last position, then `floaty_gesture_end` — because that is the
 * path under test. A pointer drag is not usable for this: whether it grabs depends on the
 * widget's own layout (a note's middle is its text area, the visualizer has no title bar at
 * all), and a drag that never started looks exactly like a drag that could not be stranded.
 *
 * Usage: node stranded.mjs [id] [port]
 */
import { APP_PORT, Session, listTargets } from "./cdp.mjs";

let id = process.argv[2];
const port = Number(process.argv[3] ?? APP_PORT);
const overlays = () =>
  listTargets(port).then((t) => t.filter((x) => x.type === "page" && x.url.endsWith("#/overlay")));

let session = null;
if (!id || id.startsWith("--")) {
  const probe = await Session.open(port, "#/overlay", 0);
  const panel = await probe.evaluate(`const l = await window.__TAURI_INTERNALS__.invoke("floaty_list");
    const kinds = ["sysmon", "clock", "note", "visualizer", "pet"];
    const p = l.find(r => kinds.includes(r.kind));
    return p ? p.id : null;`);
  await probe.close();
  if (!panel) {
    console.error("stranded: no panel on this desktop (sysmon, clock, note, visualizer or pet)");
    process.exit(2);
  }
  id = panel;
  console.log(`stranded: picked ${id}`);
}

// Any overlay window can send the release; prefer one that is already drawing the widget.
for (let nth = 0; nth < (await overlays()).length + 1 && !session; nth += 1) {
  let s;
  try {
    s = await Session.open(port, "#/overlay", nth);
  } catch {
    continue; // no such window
  }
  const has = await s
    .evaluate(`return document.getElementById("slot-" + ${JSON.stringify(id)}) !== null;`)
    .catch(() => false);
  if (has) session = s;
  else if (nth + 1 < (await overlays()).length) await s.close();
  else session = s; // nothing draws it — the invokes still work from here
}
const ask = (expr) => session.evaluate(expr);
const invoke = (cmd, args) =>
  ask(`return await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args ?? {})});`);
const record = () =>
  ask(`const r = (await window.__TAURI_INTERNALS__.invoke("floaty_list")).find(x => x.id === ${JSON.stringify(id)});
    return r ? [r.x, r.y] : null;`);
const list = await invoke("floaty_monitors");
// floaty_monitors answers with the logical rectangle of each screen, flat: {x, y, w, h}.
const onAnyScreen = (x, y) =>
  list.some((m) => x >= m.x && y >= m.y && x < m.x + m.w && y < m.y + m.h);

// A point belonging to no screen: a real gap between two screens if there is one, otherwise
// just above the top of everything.
const sorted = [...list].sort((a, b) => a.x - b.x);
let nowhere = null;
for (let i = 0; i + 1 < sorted.length; i += 1) {
  const gap = sorted[i + 1].x - (sorted[i].x + sorted[i].w);
  if (gap > 8) nowhere = [Math.round(sorted[i].x + sorted[i].w + gap / 2), Math.round(sorted[i].y + 60)];
}
if (!nowhere) nowhere = [Number(process.argv[4] ?? 120), -20];
console.log(`stranded: dropping ${id} at ${nowhere}, which is on no screen: ${!onAnyScreen(...nowhere)}`);
if (onAnyScreen(...nowhere)) {
  console.error("stranded: the chosen point is on a screen — nothing to check");
  process.exit(2);
}

const before = await record();
const depthBefore = await ask(`return (await window.__TAURI_INTERNALS__.invoke("floaty_undo_state")).depth;`);
console.log(`${id} before: ${before} (undo depth ${depthBefore})`);

// The page's release, command for command.
await invoke("floaty_gesture_begin", { label: "move", ids: [id] });
await invoke("floaty_drag_to", { id, x: nowhere[0], y: nowhere[1] });
const strandedAt = await record();
await invoke("floaty_gesture_end");
await new Promise((r) => setTimeout(r, 400));
const after = await record();
console.log(`  dragged to ${strandedAt} (on a screen: ${onAnyScreen(...strandedAt)}), released`);

// The app says so itself, in its own words: the re-home fired for this widget. A "same place
// as before" reading cannot show that on its own — a drag that moved nothing looks identical
// to a drag that left the desktop and was brought back.
// floaty_diagnostics carries the log as {path, bytes, lines[], rotated}.
const tail = await ask(`const d = await window.__TAURI_INTERNALS__.invoke("floaty_diagnostics");
  return d.log.lines.slice(-60);`);
const brought = (Array.isArray(tail) ? tail : []).filter((l) => l.includes("brought") && l.includes(id));
console.log(`  the app's own log: ${brought.length ? brought[brought.length - 1].split("] ").pop() : "nothing about " + id}`);
console.log(`${id} after the release: ${after}`);
console.log(`  on a screen: ${onAnyScreen(...after)}`);

// Who draws it now? A record no window has a slot for is invisible *and* unclickable.
let drawn = 0;
for (let nth = 0; nth < (await overlays()).length; nth += 1) {
  const s = await Session.open(port, "#/overlay", nth).catch(() => null);
  if (!s) continue;
  const has = await s
    .evaluate(`return document.getElementById("slot-" + ${JSON.stringify(id)}) !== null;`)
    .catch(() => false);
  if (has) drawn += 1;
  await s.close();
}
console.log(`  drawn by ${drawn} window(s)`);

// And the step it pushed has to hold the place it ended up, not the void: undo goes back to
// where the drag started, and redo must not put it back into the band. The probe leaves the
// stack exactly as it found it — a check that changes the desktop it is checking is no good
// to anyone running it twice.
// A person using this desktop has their own steps on the same stack (Ctrl+Alt+Z, a drag of
// their own), and one of those can land between these calls. Losing the round trip is a note,
// not a failure: what the fix promises is where the widget ends up, and that is checked
// whether or not the stack stayed still.
let undone = null;
let redone = null;
let settled = null;
try {
  await invoke("floaty_undo");
  await new Promise((r) => setTimeout(r, 300));
  undone = await record();
  await invoke("floaty_redo");
  await new Promise((r) => setTimeout(r, 300));
  redone = await record();
  await invoke("floaty_undo");
  await new Promise((r) => setTimeout(r, 300));
  settled = await record();
} catch (err) {
  console.log(`  note: the undo/redo round trip was interrupted — ${String(err).split("\n")[0]}`);
}
const depthAfter = await ask(`return (await window.__TAURI_INTERNALS__.invoke("floaty_undo_state")).depth;`);
const steps = depthAfter - depthBefore;
console.log(`${id} undo -> ${undone}, redo -> ${redone}, undo again -> ${settled} (undo depth ${depthAfter})`);

const problems = [];

// ---- and the way a person does it: a pointer drag on a real title bar -----------------
// The pass above places the widget during a drag and then asks for the re-home, which is the
// order the backend wants. The page's own release is not that order: it ends the gesture and
// *then* places the widget one last time, so a real drag could leave a widget off-screen with
// the log claiming it had been brought back. Nothing but a real pointer drag exercises that.
{
  const barPanel = await ask(`const l = await window.__TAURI_INTERNALS__.invoke("floaty_list");
    const kinds = ["sysmon", "clock", "note", "countdown", "pet"];
    const ps = l.filter(r => kinds.includes(r.kind)).sort((a, b) => b.y - a.y);
    return ps.length ? ps[0].id : null;`);
  if (!barPanel) {
    console.log("  pointer pass: no panel with a title bar to drag");
  } else {
    // It may be drawn by the other window; drive the drag where its slot actually is.
    let win = null;
    for (let nth = 0; nth < (await overlays()).length && !win; nth += 1) {
      const s = await Session.open(port, "#/overlay", nth).catch(() => null);
      if (!s) continue;
      const has = await s
        .evaluate(`return document.getElementById("slot-" + ${JSON.stringify(barPanel)}) !== null;`)
        .catch(() => false);
      if (has) win = s;
      else await s.close();
    }
    if (!win) {
      console.log(`  pointer pass: nothing draws ${barPanel}`);
    } else {
      const start = await win.evaluate(
        `const r = (await window.__TAURI_INTERNALS__.invoke("floaty_list")).find(x => x.id === ${JSON.stringify(barPanel)});
         return r ? [r.x, r.y] : null;`,
      );
      const bar = await win.evaluate(`(() => {
        const el = document.getElementById("slot-" + ${JSON.stringify(barPanel)});
        const target = el.querySelector(".bar") ?? el;
        const r = target.getBoundingClientRect();
        return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) };
      })()`);
      const mouse = (type, x, y, buttons) =>
        win.send("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons, clickCount: 1 });
      await mouse("mousePressed", bar.x, bar.y, 1);
      for (const y of [Math.max(1, bar.y - 30), 8, 2, 0]) await mouse("mouseMoved", bar.x, y, 1);
      await mouse("mouseReleased", bar.x, 0, 0);
      await new Promise((r) => setTimeout(r, 500));
      const landed = await win.evaluate(
        `const r = (await window.__TAURI_INTERNALS__.invoke("floaty_list")).find(x => x.id === ${JSON.stringify(barPanel)});
         return r ? [r.x, r.y] : null;`,
      );
      // And the app has to say it did the bringing back: ending on a screen edge could also
      // mean the drag never left, which is the false pass this pass exists to avoid.
      const after = await win.evaluate(`const d = await window.__TAURI_INTERNALS__.invoke("floaty_diagnostics");
        return d.log.lines.slice(-40);`);
      const said = (Array.isArray(after) ? after : []).filter(
        (l) => l.includes("brought") && l.includes(barPanel),
      );
      console.log(`  pointer pass: ${barPanel} dragged off the top edge from ${start} -> ${landed}` +
        (said.length ? ` — the app brought it back` : ""));
      if (!said.length) {
        problems.push(`a real pointer drag left ${barPanel} off the top edge and the app never brought it back`);
      }
      if (!onAnyScreen(...landed)) {
        problems.push(`a real pointer drag left ${barPanel} at ${landed}, on no screen`);
      } else {
        // Put it back with the step the drag itself pushed — one undo, not a second move, so
        // the probe leaves the stack no deeper than it found it.
        await win.evaluate(`await window.__TAURI_INTERNALS__.invoke("floaty_undo");`);
        await new Promise((r) => setTimeout(r, 300));
      }
      await win.close();
    }
  }
}

if (!brought.length) {
  problems.push(`the app never reported bringing ${id} back from a point on no screen`);
}
if (!onAnyScreen(...after)) {
  problems.push(`the release left ${id} at ${after}, which is on no screen at all`);
}
if (drawn === 0) {
  problems.push(`nothing is drawing ${id} after the release — invisible and unclickable`);
}
// Only ever a note: the depth cannot be attributed while a person is dragging things on the
// same desktop, and their steps land between the probe's. What is checked above is what the
// fix is about — where the widget ends up, who draws it, and where undo and redo take it.
if (steps !== 1) {
  console.log(`  note: the undo stack moved ${steps} step(s) across the release (other drags count too)`);
}
if (undone && JSON.stringify(undone) !== JSON.stringify(before)) {
  problems.push(`undo did not put it back where the drag began: ${before} -> ${undone}`);
}
if (settled && JSON.stringify(settled) !== JSON.stringify(before)) {
  problems.push(`the probe did not leave ${id} where it found it: ${before} -> ${settled}`);
}
if (redone && !onAnyScreen(...redone)) {
  problems.push(`redo put ${id} back into the void at ${redone} — the step holds the wrong state`);
}
// A note, for the same reason: the probe's own undo covers the step its drag pushed, but the
// total only lines up on a desktop nobody else is using.
const depthEnd = await ask(`return (await window.__TAURI_INTERNALS__.invoke("floaty_undo_state")).depth;`);
console.log(`  undo stack: ${depthBefore} before, ${depthEnd} after (the probe undoes what it drags)`);
await session.close();

if (problems.length) {
  console.error(`stranded: FAILED — ${problems.length} problem(s)`);
  for (const p of problems) console.error(`  - ${p}`);
  process.exit(1);
}
console.log(`stranded: ok — ${id} went to ${strandedAt} (on no screen), came back to ${after} and is ` +
  `drawn by ${drawn} window(s)${steps ? "; undo/redo are exact" : ""}`);
