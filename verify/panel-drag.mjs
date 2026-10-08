/**
 * Drag a panel within one screen and watch the record afterwards.
 *
 * The panel snap-back is panel-specific (icons are exact), so the question is *when* the
 * position goes back: at the release (an onUp path) or a moment later (a periodic save or
 * a page-side re-apply). Sampling every 100ms answers that without guessing — the one wait
 * in this folder that is the instrument rather than a substitute for one.
 *
 * It drags a panel a person put on this desktop and leaves it wherever the drag left it.
 * The question is when the record stops moving, not where it should end up, so there is no
 * place to put it back that a second run would agree on — which is why it wants a desktop
 * with a panel on it rather than a world of its own.
 *
 * Usage: node verify/panel-drag.mjs [id] [port]   (`--port N` is the other spelling)
 *   exit 0  the panel was grabbed where a person grabs it and the record followed
 *   exit 1  the app is wrong (the drag moved nothing, or the record went away)
 *   exit 2  the probe could not run (no app on the port, no panel, nothing drawing it)
 */
import { APP_PORT } from "./cdp.mjs";
import { attach, flag, probe } from "./runtime.mjs";

/** The kinds that are panels rather than tiles — the snap-back this watches is theirs. */
const PANELS = ["sysmon", "clock", "visualizer", "note"];

const port = Number(flag("port", process.argv[3] ?? APP_PORT));
const { check, skip, finish } = probe("panel-drag");

// A port that answers is not a live app: killing the app leaves a WebView2 serving the port,
// targets listed and URLs right. `attach` is what tells those apart, and says which it found.
const app = await attach({ port }).catch((error) => skip(error.message));

let id = process.argv[2];
if (!id || id.startsWith("--")) {
  // No id: take the first panel. The snap-back was panel-only, so a panel is the point.
  const panel = (await app.list()).find((r) => PANELS.includes(r.kind));
  if (!panel) skip("no panel on this desktop (sysmon, clock, note or visualizer)");
  id = panel.id;
  console.log(`panel-drag: picked ${id}`);
}

// Pick the window by what it draws, not by an index: the overlay targets are not in the
// same order in every listing, and an off-by-one here silently tests the wrong window.
const drawn = await app.drawnBy(id);
if (!drawn.length) skip(`no overlay page is drawing ${id}`);
const nth = drawn[0];

/** Two places, compared by their numbers: an array is two objects and never equal. */
const elsewhere = (a, b) => Boolean(a) && Boolean(b) && (a[0] !== b[0] || a[1] !== b[1]);

// Grab it where a person would: a panel with a title bar is dragged by the bar (the middle
// of a note is its text area, and dragging text must not move the note — the app is right
// to refuse, and a probe that grabs the middle proves nothing).
const box = await app.grabPoint(id, nth);
if (!box) skip(`no overlay page is drawing ${id}`);
console.log(`grabbing ${box.grabbed}`);

const before = await app.placeOf(id);
console.log(`${id} before: ${before}`);

// The mouse events go to the page that draws the panel, and the move is stepped so the
// page sees a drag and not one jump — the `nth` is not a detail, because a window that is
// not drawing the panel proves nothing.
await app.mouse("mousePressed", box.x, box.y, nth);
await app.moveTo(box.x + 200, box.y + 80, { from: box, nth, steps: 6 });
const during = await app.placeOf(id);
await app.mouse("mouseReleased", box.x + 200, box.y + 80, nth);

// The stopwatch. The probe is measuring *when* the record settles, so the delay between
// samples is the reading itself, not a guess at how long the app takes. Every other wait in
// this folder is a predicate with a deadline.
const samples = [];
for (let i = 0; i < 12; i += 1) {
  await new Promise((r) => setTimeout(r, 100));
  samples.push(await app.placeOf(id));
}
console.log(`during drag: ${during}`);
console.log("after release, every 100ms:");
for (const [i, sample] of samples.entries()) {
  console.log(`  ${(i + 1) * 100}ms: ${JSON.stringify(sample)}`);
}

// The premises of the timeline above. A drag that never started, and a record that went away
// mid-drag, both sample a panel that is not moving — which reads exactly like a panel that
// has already settled, so "the drag happened" has to hold before "the timing is the answer"
// means anything. Where it ends up is deliberately not asserted: the snap-back is
// panel-specific and the question is when it happens, not what the answer is.
check(before !== null, `the store holds ${id}`);
check(
  elsewhere(before, during),
  `the record followed the pointer during the drag (${JSON.stringify(before)} -> ${JSON.stringify(during)})`,
);
check(
  samples.every((s) => s !== null),
  "the record survived the release",
);

// Which sample was the last one that changed anything: that is when it settled. Printed and
// not asserted, because the answer belongs to the machine — a probe that demanded a
// particular millisecond would be failing on the hardware.
const settled = samples.findLastIndex((s, i) => i > 0 && elsewhere(samples[i - 1], s)) + 1;
console.log(
  `panel-drag: ${id} last moved at ${
    settled
      ? `sample ${settled} (${settled * 100}ms after the release)`
      : "no sample — it was where it ended"
  }`,
);

await finish(() => app.close());
