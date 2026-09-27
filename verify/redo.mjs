/**
 * Redo: undoing a merge and then redoing it has to put the same file back in the same
 * folder, and the folder must not move either way.
 *
 * The whole point of the design is that nothing recorded the "after" of an action when it
 * ran — the undo is the only moment it exists — so this drives the real commands and checks
 * the disk, not just the records.
 *
 * It runs in a fixture world of its own (see `runtime.mjs`): the desktop it merges on is a
 * scratch folder, the store is scratch too, and the log is written there. That is what makes
 * "the probe put the desktop back" an exact assertion rather than a hopeful one — and it is
 * why the stack it drains at the end is nobody's but its own.
 *
 * Usage: node verify/redo.mjs [--keep] [--port N]
 */
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import os from "node:os";
import { APP_PORT } from "./cdp.mjs";
import { Fixture, flag, launch, probe, samePath } from "./runtime.mjs";

const keep = process.argv.includes("--keep");
const port = Number(flag("port", APP_PORT));
const { check, skip, finish, problems } = probe("redo");

const fixture = Fixture.create({ dir: join(os.tmpdir(), `floaty-redo-${process.pid}`) });

const app = await launch({ fixture, cwd: resolve(import.meta.dirname, ".."), port }).catch((error) =>
  skip(error.message),
);

// After `launch`, not before: exit handlers run in the order they were registered, and the
// one that stops the app has to run first — remove the folder while the app is still writing
// and it is back a moment later, with a fresh log in it.
process.on("exit", () => {
  if (!keep) fixture.remove();
});

const state = () => app.invoke("floaty_undo_state");

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
    // The app's watcher debounces a new file for a few seconds before it syncs; a
    // short deadline here fails the probe on the app being careful.
    { timeout: 60 },
  );

/** Wait for the undo state to satisfy `want`, and hand the state back. */
const settled = (want, what) =>
  app.until(what, async () => {
    const s = await state();
    return want(s) ? s : null;
  });

// The probe's own two artifacts, in its own root — which also exercises the watcher
// noticing a file it did not know about.
const stamp = String(process.pid);
const fileName = `zz-redo-${stamp}.txt`;
const folderName = `zz-redo-folder-${stamp}`;
const filePath = fixture.inRoot(fileName);
const folderPath = fixture.inRoot(folderName);
writeFileSync(filePath, "floaty redo probe\n");
mkdirSync(folderPath, { recursive: true });
console.log(`redo: world ${fixture.root}`);

const file = await byPath(filePath, true);
const folder = await byPath(folderPath, true);
if (!file || !folder) skip("the two probe artifacts did not become floaties");
const fileAt = [file.x, file.y];
const folderAt = [folder.x, folder.y];
console.log(`redo: ${file.id} at ${fileAt}, ${folder.id} at ${folderAt}`);

/** The folder's item list: an item's `target` is where the file is *now*, inside the folder. */
const items = async () => {
  const rec = (await app.list()).find((r) => r.id === folder.id);
  return (rec?.data?.items ?? []).map((it) => it.name ?? "");
};
const folderStill = async () => {
  const rec = (await app.list()).find((r) => r.id === folder.id);
  return rec?.x === folderAt[0] && rec?.y === folderAt[1];
};

// ---- the merge: drag the file onto the folder's tile and drop it there ----------------
await app.invoke("floaty_drag_to", { id: file.id, x: folderAt[0] + 40, y: folderAt[1] + 50 });
await app.invoke("floaty_dropped", { id: file.id, x: folderAt[0] + 40, y: folderAt[1] + 50 });
check((await byPath(filePath, false)) === true, "the merge took the file's record away");
check((await items()).includes(fileName), `the folder holds ${fileName} (items: ${JSON.stringify(await items())})`);
check(existsSync(join(folderPath, fileName)), "the file is inside the folder on disk");
check(
  (await settled((s) => s.depth >= 1 && s.redo_depth === 0, "one step to undo, nothing to redo")) !== null,
  `one step to undo, nothing to redo (${JSON.stringify(await state())})`,
);

// ---- undo: the file comes home, the folder does not move ------------------------------
await app.invoke("floaty_undo");
const back = await byPath(filePath, true);
check(back !== null && back !== true, "undo brought the file's record back");
if (back && back !== true) {
  check(back.x === fileAt[0] && back.y === fileAt[1], `it came back to ${fileAt} (was ${back.x},${back.y})`);
}
check(!existsSync(join(folderPath, fileName)), "the file left the folder on disk");
check(await folderStill(), "the folder did not move");
check(
  (await settled((s) => s.redo_depth === 1, "the undo to make one redo available")) !== null,
  `the undo made one redo available (${JSON.stringify(await state())})`,
);

// ---- redo: the same file, the same folder, and the folder still does not move ---------
await app.invoke("floaty_redo");
check((await byPath(filePath, false)) === true, "redo took the file's record away again");
check((await items()).includes(fileName), `the folder holds ${fileName} again (items: ${JSON.stringify(await items())})`);
check(existsSync(join(folderPath, fileName)), "the file is inside the folder on disk again");
check(await folderStill(), "the folder still did not move");
check(
  (await settled((s) => s.redo_depth === 0 && s.depth >= 1, "redo to hand the step back to undo")) !== null,
  `redo handed the step back to undo (${JSON.stringify(await state())})`,
);

// ---- and back once more: the two keys trade the same step ----------------------------
await app.invoke("floaty_undo");
const home2 = await byPath(filePath, true);
check(home2 !== null && home2 !== true && home2.x === fileAt[0] && home2.y === fileAt[1], "undo put it home a second time");
check(
  (await settled((s) => s.redo_depth === 1, "the redo to be available again")) !== null,
  "and the redo is available again",
);

// ---- a new action makes the future unreachable ---------------------------------------
await app.tryInvoke("floaty_undo_checkpoint", { label: "redo-probe", restore: [] });
check(
  (await settled((s) => s.redo_depth === 0, "a new action to clear what was left to redo")) !== null,
  "a new action cleared what was left to redo",
);
const gone = await app.tryInvoke("floaty_redo");
check(typeof gone?.error === "string" && gone.error.includes("nothing left to redo"), `redo now refuses: ${gone?.error ?? "no error"}`);

// ---- put the world back, and leave the history empty ---------------------------------
// The stack is the fixture's own, so unwinding all of it is safe here — a probe against
// the user's install cannot say that, which is the other reason this one has a world.
if (!keep) {
  await app.until("the undo stack to empty", async () => {
    const s = await state();
    if (!s.depth) return true;
    await app.tryInvoke("floaty_undo");
    return null;
  });
  rmSync(filePath, { force: true });
  rmSync(join(folderPath, fileName), { force: true });
  rmSync(folderPath, { force: true, recursive: true });
  await byPath(filePath, false);
  await byPath(folderPath, false);
}
const final = await state();
console.log(`redo: done — ${problems.length} problem(s); stack depth ${final.depth}, redo depth ${final.redo_depth}`);

await finish(() => app.stop(), () => (keep ? undefined : fixture.remove()));
