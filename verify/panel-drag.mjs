/**
 * Drag a panel within one screen and watch the record afterwards.
 *
 * The panel snap-back is panel-specific (icons are exact), so the question is *when* the
 * position goes back: at the release (an onUp path) or a moment later (a periodic save or
 * a page-side re-apply). Sampling every 100ms answers that without guessing.
 *
 * Usage: node panel-drag.mjs <id> [port]
 */
import { APP_PORT, Session, listTargets } from "./cdp.mjs";

let id = process.argv[2];
const port = Number(process.argv[3] ?? APP_PORT);
if (!id || id.startsWith("--")) {
  // No id: take the first panel. The snap-back was panel-only, so a panel is the point.
  const probe = await Session.open(port, "#/overlay", 0);
  const panel = await probe.evaluate(`const l = await window.__TAURI_INTERNALS__.invoke("floaty_list");
    const kinds = ["sysmon", "clock", "visualizer", "pet", "note"];
    const p = l.find(r => kinds.includes(r.kind));
    return p ? p.id : null;`);
  await probe.close();
  if (!panel) {
    console.error("panel-drag: no panel on this desktop (sysmon, clock, note, visualizer or pet)");
    process.exit(2);
  }
  id = panel;
  console.log(`panel-drag: picked ${id}`);
}

const targets = (await listTargets(port)).filter(
  (t) => t.type === "page" && t.url.endsWith("#/overlay"),
);
// Pick the window by what it draws, not by an index: the overlay targets are not in the
// same order in every listing, and an off-by-one here silently tests the wrong window.
let session = null;
for (let nth = 0; nth < targets.length + 1; nth += 1) {
  let s;
  try {
    s = await Session.open(port, "#/overlay", nth);
  } catch {
    continue; // no such window
  }
  const has = await s.evaluate(`return document.getElementById("slot-" + ${JSON.stringify(id)}) !== null;`).catch(() => false);
  if (has) {
    session = s;
    break;
  }
  await s.close();
}
if (!session) {
  console.error(`no overlay page is drawing ${id}`);
  process.exit(2);
}

const at = () =>
  session.evaluate(`const r = (await window.__TAURI_INTERNALS__.invoke("floaty_list")).find(x => x.id === ${JSON.stringify(id)});
    return r ? [r.x, r.y] : null;`);

// Grab it where a person would: a panel with a title bar is dragged by the bar (the middle
// of a note is its text area, and dragging text must not move the note — the app is right
// to refuse, and a probe that grabs the middle proves nothing).
const box = await session.evaluate(`(() => {
  const el = document.getElementById("slot-" + ${JSON.stringify(id)});
  const target = el.querySelector(".bar") ?? el;
  const r = target.getBoundingClientRect();
  return {
    x: Math.round(r.x + r.width / 2),
    y: Math.round(r.y + r.height / 2),
    grabbed: target === el ? "the panel" : "its title bar",
  };
})()`);
console.log(`grabbing ${box.grabbed}`);

const before = await at();
console.log(`${id} before: ${before}`);

const send = (type, x, y) =>
  session.send("Input.dispatchMouseEvent", {
    type,
    x: Math.round(x),
    y: Math.round(y),
    button: "left",
    buttons: type === "mouseReleased" ? 0 : 1,
    clickCount: type === "mouseReleased" ? 1 : 0,
  });

await send("mousePressed", box.x, box.y);
for (let step = 1; step <= 6; step += 1) {
  await send("mouseMoved", box.x + (200 * step) / 6, box.y + (80 * step) / 6);
}
const during = await at();
await send("mouseReleased", box.x + 200, box.y + 80);

const samples = [];
for (let i = 0; i < 12; i += 1) {
  await new Promise((r) => setTimeout(r, 100));
  samples.push(await at());
}
console.log(`during drag: ${during}`);
console.log("after release, every 100ms:");
for (const [i, sample] of samples.entries()) {
  console.log(`  ${(i + 1) * 100}ms: ${JSON.stringify(sample)}`);
}
await session.close();
