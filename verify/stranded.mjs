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
 * check. Two screens at different scales is what makes the band real, so on a desktop whose
 * logical rectangles touch there is no void to drop into and the probe has nothing to check.
 *
 * The release is driven with the same two commands the page's own release sends —
 * `floaty_drag_to` for the last position, then `floaty_gesture_end` — because that is the
 * path under test. A pointer drag is not usable for the first pass: whether it grabs depends
 * on the widget's own layout (a note's middle is its text area, the visualizer has no title
 * bar at all), and a drag that never started looks exactly like a drag that could not be
 * stranded. The second pass is a real pointer drag, on a panel, for the reason its comment
 * gives.
 *
 * It moves a widget a person put on this desktop and undoes its own steps afterwards, so
 * nothing is left moved — but the undo stack is shared with the person using the machine,
 * which is why the depth readings are notes and not checks.
 *
 * Usage: node verify/stranded.mjs [id] [port] [x]   (`--port N` is the other spelling)
 *   exit 0  every drop the probe made came back onto a screen
 *   exit 1  the app is wrong (a widget left in the void, invisible, or a bad undo step)
 *   exit 2  the probe could not run (no app on the port, no panel, no band to drop into)
 */
import { APP_PORT } from "./cdp.mjs";
import { attach, flag, probe } from "./runtime.mjs";

/** The kinds that are panels rather than tiles — the release path being tested is theirs. */
const PANELS = ["sysmon", "clock", "note", "visualizer"];
/** The panel the pointer pass drags, and the only one with a countdown to fall back on. */
const BARRED = ["sysmon", "clock", "note", "countdown"];

const port = Number(flag("port", process.argv[3] ?? APP_PORT));
const { check, skip, finish } = probe("stranded");

const app = await attach({ port }).catch((error) => skip(error.message));

/** How deep the undo stack is right now. */
const depth = async () => (await app.invoke("floaty_undo_state"))?.depth ?? null;
/** The size of a slot as the page has it drawn — the whole body, not the title bar. */
const size = (which, nth) =>
  app.page(nth).evaluate(`(() => {
    const el = document.getElementById("slot-" + ${JSON.stringify(which)});
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return [Math.round(r.width), Math.round(r.height)];
  })()`);
/** The page's own release, command for command. */
const release = (which, x, y) =>
  app
    .tryInvoke("floaty_gesture_begin", { label: "move", ids: [which] })
    .then(() => app.tryInvoke("floaty_drag_to", { id: which, x, y }));
/** The log's own words about this widget coming back. */
const said = async (which, n = 60) =>
  (await app.logLines(n)).filter((l) => l.includes("brought") && l.includes(which));

let id = process.argv[2];
if (!id || id.startsWith("--")) {
  const panel = (await app.list()).find((r) => PANELS.includes(r.kind));
  if (!panel) skip("no panel on this desktop (sysmon, clock, note or visualizer)");
  id = panel.id;
  console.log(`stranded: picked ${id}`);
}

// The screens, as flat logical rectangles — floaty_monitors answers `{x, y, w, h}` each, and
// the pass below wants the same numbers `floaty_screens` would give with a different shape.
const list = await app.monitors();
// The invariant the app promises, and the one `screen_of` alone cannot express: not just the
// widget's corner on a screen, its whole body. The part past an edge is drawn by nobody — an
// overlay window is exactly one screen — so it is invisible even though the corner is fine.
const insideOneScreen = (x, y, w, h) =>
  list.some((s) => x >= s.x && y >= s.y && x + w <= s.x + s.w && y + h <= s.y + s.h);
const onAnyScreen = (x, y) =>
  list.some((m) => x >= m.x && y >= m.y && x < m.x + m.w && y < m.y + m.h);

// A point belonging to no screen: a real gap between two screens if there is one, otherwise
// just above the top of everything.
const sorted = [...list].sort((a, b) => a.x - b.x);
let nowhere = null;
for (let i = 0; i + 1 < sorted.length; i += 1) {
  const gap = sorted[i + 1].x - (sorted[i].x + sorted[i].w);
  if (gap > 8)
    nowhere = [Math.round(sorted[i].x + sorted[i].w + gap / 2), Math.round(sorted[i].y + 60)];
}
if (!nowhere) nowhere = [Number(process.argv[4] ?? 120), -20];
console.log(
  `stranded: dropping ${id} at ${nowhere}, which is on no screen: ${!onAnyScreen(...nowhere)}`,
);
if (onAnyScreen(...nowhere)) skip("the chosen point is on a screen — nothing to check");

const before = await app.placeOf(id);
const depthBefore = await depth();
console.log(`${id} before: ${before} (undo depth ${depthBefore})`);

// The page's release, command for command.
const placed = await release(id, nowhere[0], nowhere[1]);
check(
  !placed?.error,
  `the page's own release placed the widget in the band (${placed?.error ?? "placed"})`,
);
const strandedAt = await app.placeOf(id);
await app.tryInvoke("floaty_gesture_end");
// Wait for the re-home, not for a guess at how long it takes. A timeout is not the end of the
// probe: the position it gives up on is read anyway and the checks below report it, so a
// broken app fails here as a broken app.
await app
  .until("the release to bring the widget back onto a screen", async () => {
    const at = await app.placeOf(id);
    return at && onAnyScreen(...at) ? at : null;
  })
  .catch(() => null);
const after = await app.placeOf(id);
console.log(
  `  dragged to ${strandedAt} (on a screen: ${strandedAt && onAnyScreen(...strandedAt)}), released`,
);

// The app says so itself, in its own words: the re-home fired for this widget. A "same place
// as before" reading cannot show that on its own — a drag that moved nothing looks identical
// to a drag that left the desktop and was brought back.
const brought = await said(id);
console.log(
  `  the app's own log: ${brought.length ? brought[brought.length - 1].split("] ").pop() : `nothing about ${id}`}`,
);
console.log(`${id} after the release: ${after}`);
console.log(`  on a screen: ${after && onAnyScreen(...after)}`);
// Who draws it now? A record no window has a slot for is invisible *and* unclickable. The
// window it answers with is also the one whose idea of the body is worth measuring: a slot
// on another screen would be a different widget's idea of the same id.
const drawing = await app.drawnBy(id);
const drawn = drawing.length;
const window0 = drawing[0] ?? 0;
console.log(`  drawn by ${drawn} window(s)`);
const body = await size(id, window0);
console.log(
  `  body ${body?.[0]}x${body?.[1]}: wholly on one screen: ${body && insideOneScreen(...after, ...body)}`,
);

// And the step it pushed has to hold the place it ended up, not the void: undo goes back to
// where the drag started, and redo must not put it back into the band. The probe leaves the
// stack exactly as it found it — a check that changes the desktop it is checking is no good
// to anyone running it twice.
// A person using this desktop has their own steps on the same stack (Ctrl+Alt+Z, a drag of
// their own), and one of those can land between these calls, which is why the depth is a
// note rather than a check. A round trip that did not run is not a note: the check below
// asks for it, so a broken undo cannot pass by leaving nothing to assert.
/**
 * Where a record is once a move has landed: wait for it to differ from where it was, and read
 * it at the deadline if it never does. A step that changes nothing is a real answer, so a
 * wait on "it moved" alone would call a refused undo a slow one.
 */
const afterStep = (what, which = id, from = null) =>
  app
    .until(`${what} to land`, async () => {
      const at = await app.placeOf(which);
      if (!at) return null;
      return !from || at[0] !== from[0] || at[1] !== from[1] ? at : null;
    })
    .catch(() => app.placeOf(which));
let undone = null;
let redone = null;
let settled = null;
try {
  const back = await app.tryInvoke("floaty_undo");
  if (back?.error) throw new Error(back.error);
  undone = await afterStep("the undo", id, after);
  const again = await app.tryInvoke("floaty_redo");
  if (again?.error) throw new Error(again.error);
  redone = await afterStep("the redo", id, undone);
  const rest = await app.tryInvoke("floaty_undo");
  if (rest?.error) throw new Error(rest.error);
  settled = await afterStep("the second undo", id, redone);
} catch (err) {
  console.log(`  note: the undo/redo round trip was interrupted — ${String(err).split("\n")[0]}`);
}
const depthAfter = await depth();
const steps = depthAfter - depthBefore;
console.log(
  `${id} undo -> ${undone}, redo -> ${redone}, undo again -> ${settled} (undo depth ${depthAfter})`,
);

// ---- and the way a person does it: a pointer drag on a real title bar -----------------
// The pass above places the widget during a drag and then asks for the re-home, which is the
// order the backend wants. The page's own release is not that order: it ends the gesture and
// *then* places the widget one last time, so a real drag could leave a widget off-screen with
// the log claiming it had been brought back. Nothing but a real pointer drag exercises that.
{
  // The topmost barred panel, so a drag to the top edge has the furthest to travel and the
  // release is a real gesture rather than a one-pixel move.
  const barPanel = (await app.list())
    .filter((r) => BARRED.includes(r.kind))
    .sort((a, b) => b.y - a.y)[0]?.id;
  if (!barPanel) {
    console.log("  pointer pass: no panel with a title bar to drag");
  } else {
    // It may be drawn by the other window; drive the drag where its slot actually is.
    const barDrawn = await app.drawnBy(barPanel);
    if (!barDrawn.length) {
      console.log(`  pointer pass: nothing draws ${barPanel}`);
    } else {
      const nth = barDrawn[0];
      // Not `App.mouse`: that one derives `buttons` and `clickCount` from the event type,
      // and this pass needs a press–move–release the page reads as one gesture, with the
      // button state spelled out for every event. What `App.mouse` has that this borrows is
      // the page — the events go to the window that has the slot, which is not always the
      // first one.
      const mouse = (type, x, y, buttons) =>
        app.page(nth).send("Input.dispatchMouseEvent", {
          type,
          x: Math.round(x),
          y: Math.round(y),
          button: "left",
          buttons,
          clickCount: 1,
        });
      const start = await app.placeOf(barPanel);
      const bar = await app.grabPoint(barPanel, nth);
      if (!bar) {
        console.log(`  pointer pass: ${barPanel} has no bar to grab`);
      } else {
        await mouse("mousePressed", bar.x, bar.y, 1);
        for (const y of [Math.max(1, bar.y - 30), 8, 2, 0]) await mouse("mouseMoved", bar.x, y, 1);
        await mouse("mouseReleased", bar.x, 0, 0);
        // A release is a moment, not a reading: wait for the place to change, and read it at
        // the deadline if it never does, so a slow machine is not called a missing widget.
        const landed = await afterStep("the top-edge drag", barPanel, start);
        // And the app has to say it did the bringing back: ending on a screen edge could also
        // mean the drag never left, which is the false pass this pass exists to avoid.
        const claimed = await said(barPanel, 40);
        // ...and to the right, which is what was reported: the body hangs over the edge with its
        // top-left still on the screen, so nothing objected and half the widget was drawn by
        // nobody. The edge is measured after the drag above, which moved the panel: an edge
        // worked out from where it used to be is one it may already be past.
        const box2 = await size(barPanel, nth);
        const now = await app.grabPoint(barPanel, nth);
        if (!now) {
          console.log(`  pointer pass: ${barPanel} is not mounted where it was a moment ago`);
        } else {
          const mine = list.find((m) => now.x >= m.x && now.x < m.x + m.w);
          const edge = Math.round((mine?.x ?? 0) + (mine?.w ?? 1920) - 4);
          await mouse("mousePressed", now.x, now.y, 1);
          for (const x of [now.x + Math.round((edge - now.x) / 2), edge - 30, edge, edge]) {
            await mouse("mouseMoved", x, now.y, 1);
          }
          await mouse("mouseReleased", edge, now.y, 0);
          const right = await afterStep("the right-edge drag", barPanel, landed);
          const okBody = Boolean(right && box2 && insideOneScreen(...right, ...box2));
          console.log(
            `  pointer pass: ${barPanel} dragged to the right edge -> ${right} ` +
              `(body ${box2?.[0]}x${box2?.[1]}, wholly on one screen: ${okBody})`,
          );
          check(
            okBody,
            `dragging to the right edge left ${barPanel}'s body on the screen: at ${right}, body ${box2?.[0]}x${box2?.[1]}`,
          );
          console.log(
            `  pointer pass: ${barPanel} dragged off the top edge from ${start} -> ${landed}` +
              (claimed.length ? ` — the app brought it back` : ""),
          );
          if (!claimed.length) {
            // Not a failure any more: the whole body is clamped during the drag now, so the widget
            // may never have left a screen at all — which is the better of the two outcomes. What
            // has to hold either way is where its body is.
            console.log("  (nothing needed bringing back — the body never left a screen)");
          }
          const landedSize = await size(barPanel, nth);
          const okLanded = Boolean(
            landed && landedSize && insideOneScreen(...landed, ...landedSize),
          );
          check(
            okLanded,
            `a real pointer drag left ${barPanel}'s body on the desktop: at ${landed}, body ${landedSize?.[0]}x${landedSize?.[1]}`,
          );
          if (okLanded) {
            // Put it back with the step the drag itself pushed — one undo, not a second move, so
            // the probe leaves the stack no deeper than it found it.
            const back = await app.tryInvoke("floaty_undo");
            if (back?.error) {
              console.log(`  note: the drag's own step would not undo — ${back.error}`);
            } else {
              await afterStep("the drag's own undo", barPanel, right);
            }
          }
        }
      }
    }
  }
}

// The re-home is not something this run can force any more: the drag clamps the whole body
// to a screen, so the widget very often never leaves one, and the app has nothing to bring
// back. The checks below say what has to hold either way — where the body is, that a window
// draws it, and that undo and redo put it back. The log line is evidence when it is there,
// not a requirement the app can still meet.
if (!brought.length) {
  console.log(`  (nothing to bring back — ${id} never left a screen)`);
}
check(Boolean(after && onAnyScreen(...after)), `the release left ${id} on a screen (at ${after})`);
check(
  Boolean(after && body && insideOneScreen(...after, ...body)),
  `the release left ${id}'s whole body on one screen (at ${after}, body ${body?.[0]}x${body?.[1]})`,
);
check(drawn > 0, `something is drawing ${id} after the release (${drawn} window(s))`);
// Only ever a note: the depth cannot be attributed while a person is dragging things on the
// same desktop, and their steps land between the probe's. What is checked above is what the
// fix is about — where the widget ends up, who draws it, and where undo and redo take it.
if (steps !== 1) {
  console.log(
    `  note: the undo stack moved ${steps} step(s) across the release (other drags count too)`,
  );
}
// That the round trip ran at all is a check of its own. The three below all read "no value,
// so nothing to say", which on a run where `floaty_undo` threw is not a pass but a probe
// that asked nothing: an app whose undo stack is gone would report a clean run.
check(
  undone !== null && redone !== null && settled !== null,
  `the undo/redo round trip ran (undo ${undone}, redo ${redone}, undo again ${settled})`,
);
check(
  !undone || JSON.stringify(undone) === JSON.stringify(before),
  `undo put ${id} back where the drag began (${before} -> ${undone})`,
);
check(
  !settled || JSON.stringify(settled) === JSON.stringify(before),
  `the probe left ${id} where it found it (${before} -> ${settled})`,
);
check(!redone || onAnyScreen(...redone), `redo did not put ${id} back into the void (${redone})`);
// A note, for the same reason: the probe's own undo covers the step its drag pushed, but the
// total only lines up on a desktop nobody else is using.
const depthEnd = await depth();
console.log(
  `  undo stack: ${depthBefore} before, ${depthEnd} after (the probe undoes what it drags)`,
);
console.log(
  `stranded: ${id} went to ${strandedAt} (on no screen), came back to ${after} and is ` +
    `drawn by ${drawn} window(s)${steps ? "; undo/redo are exact" : ""}`,
);

await finish(() => app.close());
