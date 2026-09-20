/**
 * Redo: undoing a merge and then redoing it has to put the same file back in the same
 * folder, and the folder must not move either way.
 *
 * The whole point of the design is that nothing recorded the "after" of an action when it
 * ran — the undo is the only moment it exists — so this drives the real commands and checks
 * the disk, not just the records.
 *
 * It creates its own two artifacts (`zz-redo-*`) in the desktop root, merges one into the
 * other, and undoes/redoes around it. It never touches a file it did not make, and it
 * removes what it made.
 *
 * Usage: node verify/redo.mjs [--keep]
 */
import { mkdirSync, rmSync, existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { APP_PORT, Session } from "./cdp.mjs";

const keep = process.argv.includes("--keep");
const session = await Session.open(APP_PORT, "#/overlay", 0);

const call = (cmd, args = {}) =>
  session
    .evaluate(`return JSON.stringify(await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)}));`)
    .then((text) => JSON.parse(text));
const callSafe = (cmd, args = {}) => call(cmd, args).catch((err) => ({ error: String(err) }));
const list = () => call("floaty_list");
const state = () => call("floaty_undo_state");

const problems = [];
const check = (ok, what) => {
  console.log(`  ${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) problems.push(what);
};

// The desktop root, from a record that is already in it.
// A file floatie keeps its path under `target`; folders use `path`.
const pathOf = (r) => r.data?.target ?? r.data?.path ?? "";
const records = await list();
const anyFile = records.find((r) => r.kind !== "folder" && pathOf(r).includes("\\"));
if (!anyFile) {
  console.error("redo: no file floatie to read the desktop root from");
  process.exit(2);
}
const root = pathOf(anyFile).slice(0, pathOf(anyFile).lastIndexOf("\\"));
const stamp = String(process.pid);
const fileName = `zz-redo-${stamp}.txt`;
const folderName = `zz-redo-folder-${stamp}`;
const filePath = join(root, fileName);
const folderPath = join(root, folderName);
console.log(`redo: desktop root ${root}`);

writeFileSync(filePath, "floaty redo probe\n");
mkdirSync(folderPath, { recursive: true });

/** Wait for a record whose target is `path` (or for it to go away). */
const byPath = async (path, want = true, tries = 40) => {
  for (let i = 0; i < tries; i += 1) {
    const found = (await list()).find((r) => pathOf(r).toLowerCase() === path.toLowerCase());
    if (Boolean(found) === want) return found ?? true;
    await new Promise((r) => setTimeout(r, 250));
  }
  return null;
};

const file = await byPath(filePath, true);
const folder = await byPath(folderPath, true);
if (!file || !folder) {
  console.error("redo: the two probe artifacts did not become floaties");
  process.exit(2);
}
const fileAt = [file.x, file.y];
const folderAt = [folder.x, folder.y];
console.log(`redo: ${file.id} at ${fileAt}, ${folder.id} at ${folderAt}`);

/** The folder's item list: an item's `target` is where the file is *now*, inside the folder. */
const items = async () => {
  const rec = (await list()).find((r) => r.id === folder.id);
  return (rec?.data?.items ?? []).map((it) => it.name ?? "");
};
const folderStill = async () => (await list()).find((r) => r.id === folder.id)?.x === folderAt[0]
  && (await list()).find((r) => r.id === folder.id)?.y === folderAt[1];

// ---- the merge: drag the file onto the folder's tile and drop it there ----------------
await call("floaty_drag_to", { id: file.id, x: folderAt[0] + 40, y: folderAt[1] + 50 });
await call("floaty_dropped", { id: file.id, x: folderAt[0] + 40, y: folderAt[1] + 50 });
await new Promise((r) => setTimeout(r, 600));
check((await byPath(filePath, false)) === true, "the merge took the file's record away");
check((await items()).includes(fileName), `the folder holds ${fileName} (items: ${JSON.stringify(await items())})`);
check(existsSync(join(folderPath, fileName)), "the file is inside the folder on disk");
const afterMerge = await state();
check(afterMerge.depth >= 1 && afterMerge.redo_depth === 0, `one step to undo, nothing to redo (${JSON.stringify(afterMerge)})`);

// ---- undo: the file comes home, the folder does not move ------------------------------
await call("floaty_undo");
await new Promise((r) => setTimeout(r, 600));
const back = await byPath(filePath, true);
check(back !== null && back !== true, "undo brought the file's record back");
if (back && back !== true) {
  check(back.x === fileAt[0] && back.y === fileAt[1], `it came back to ${fileAt} (was ${back.x},${back.y})`);
}
check(!existsSync(join(folderPath, fileName)), "the file left the folder on disk");
check(await folderStill(), "the folder did not move");
const afterUndo = await state();
check(afterUndo.redo_depth === 1, `the undo made one redo available (${JSON.stringify(afterUndo)})`);

// ---- redo: the same file, the same folder, and the folder still does not move ---------
await call("floaty_redo");
await new Promise((r) => setTimeout(r, 600));
check((await byPath(filePath, false)) === true, "redo took the file's record away again");
check((await items()).includes(fileName), `the folder holds ${fileName} again (items: ${JSON.stringify(await items())})`);
check(existsSync(join(folderPath, fileName)), "the file is inside the folder on disk again");
check(await folderStill(), "the folder still did not move");
const afterRedo = await state();
check(afterRedo.redo_depth === 0 && afterRedo.depth >= 1, `redo handed the step back to undo (${JSON.stringify(afterRedo)})`);

// ---- and back once more: the two keys trade the same step ----------------------------
await call("floaty_undo");
await new Promise((r) => setTimeout(r, 600));
const home2 = await byPath(filePath, true);
check(home2 !== null && home2 !== true && home2.x === fileAt[0] && home2.y === fileAt[1], "undo put it home a second time");
check((await state()).redo_depth === 1, "and the redo is available again");

// ---- a new action makes the future unreachable ---------------------------------------
await callSafe("floaty_undo_checkpoint", { label: "redo-probe", restore: [] });
check((await state()).redo_depth === 0, "a new action cleared what was left to redo");
const gone = await callSafe("floaty_redo");
check(typeof gone?.error === "string" && gone.error.includes("nothing left to redo"), `redo now refuses: ${gone?.error ?? "no error"}`);

// ---- put the desktop back, and leave the history empty -------------------------------
for (let i = 0; i < 12; i += 1) {
  const s = await state();
  if (!s.depth) break;
  await callSafe("floaty_undo");
}
if (!keep) {
  rmSync(filePath, { force: true });
  rmSync(join(folderPath, fileName), { force: true });
  rmSync(folderPath, { force: true, recursive: true });
  await byPath(filePath, false);
  await byPath(folderPath, false);
}
const final = await state();
console.log(`redo: done — ${problems.length} problem(s); stack depth ${final.depth}, redo depth ${final.redo_depth}`);

await session.close();
process.exit(problems.length ? 1 : 0);
