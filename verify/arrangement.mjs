/**
 * A display change rearranges the desktop — but not for the shape it reported until that
 * shape has stopped changing.
 *
 * Windows reports shapes it does not stay in. Measured on this machine, in its own log: a
 * fullscreen app changed the display mode and changed it back, so `monitors: the desktop is
 * now 0,0 960x1440` was followed two and a half seconds later by `0,0 1440x960` — and the
 * desktop was rearranged for the shape in the middle, permanently, because a floatie's place
 * is pinned and nothing else in the app has an opinion about position. The report was "the
 * icons' layout is inconsistent: turn the machine off for a long time, come back, and the
 * layout is messed up".
 *
 * So the two halves of an arrangement change are timed differently, and this probe separates
 * them: the *windows* follow the report at once (an overlay is one screen, so a floatie is
 * only drawn where an overlay covers it), and the *rearrangement* waits for the shape to
 * stand still. A probe that only looked at where the floaties ended up could not tell those
 * apart — pre-fix the same end state is reached, in the same run, just instantly and for a
 * shape that was about to stop existing.
 *
 * Nothing about the display is changed: this sends the one message Windows sends, to every
 * window, with the shape the desktop is really in. The app is the thing under test, and the
 * log is where it says what it did.
 *
 * Usage: node verify/arrangement.mjs [port]
 *   exit 0  the report was refitted at once and rearranged for only after it settled
 *   exit 1  the app is wrong (acted on the report at once, or never acted on it at all)
 *   exit 2  the probe could not run (no app on the port)
 */
import { spawnSync } from "node:child_process";
import { APP_PORT } from "./cdp.mjs";
import { attach, flag, probe } from "./runtime.mjs";

/** How long the app promises to wait for a shape to stand still (`ARRANGEMENT_SETTLE_MS`). */
const SETTLE_MS = 3000;
/** The watch ticks twice a second, and a tick lands after the window, not on it. */
const TICK_SLACK_MS = 1200;

const port = Number(flag("port", process.argv[3] ?? APP_PORT));
const { check, skip, finish } = probe("arrangement");
const app = await attach({ port }).catch((error) => skip(error.message));

/** The log's millisecond stamp, as a number that subtracts. */
const stamp = (line) => {
  const m = /^\[(\d+):(\d+):(\d+)\.(\d+)\]/.exec(line.trim());
  return m ? (+m[1] * 3600 + +m[2] * 60 + +m[3]) * 1000 + +m[4] : null;
};
const saying = (lines, needle) => lines.filter((l) => l.includes(needle));

/**
 * WM_DISPLAYCHANGE to HWND_BROADCAST, wParam = bits per pixel, lParam = (h << 16) | w.
 * Encoded rather than quoted: a PowerShell one-liner is four levels of quoting, and a probe
 * that fails to *send* reads exactly like an app that failed to react.
 */
const sendDisplayChange = (w, h) => {
  const script = [
    "Add-Type -Namespace FloatyProbe -Name User32 -MemberDefinition @'",
    '[DllImport("user32.dll")] public static extern bool SendNotifyMessage(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, System.IntPtr lParam);',
    "'@",
    `$l = New-Object System.IntPtr ([int64]((${h} * 65536) + ${w}))`,
    "$wp = New-Object System.UIntPtr ([uint64]32)",
    "$hw = New-Object System.IntPtr ([int64]0xffff)",
    "Write-Output \"ok=$([FloatyProbe.User32]::SendNotifyMessage($hw, [uint32]0x7E, $wp, $l))\"",
  ].join("\n");
  const out = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-EncodedCommand",
     Buffer.from(script, "utf16le").toString("base64")],
    { encoding: "utf8" },
  );
  return { ok: /ok=True/.test(out.stdout ?? ""), stdout: (out.stdout ?? "").trim(), stderr: (out.stderr ?? "").trim().split("\n")[0] };
};

// The shape the desktop is really in, so the message carries the truth: a probe that lies
// about the shape is testing a display change that did not happen.
const monitors = await app.monitors();
if (!monitors.length) skip("the app reports no monitors");
const w = Math.round(Math.max(...monitors.map((m) => m.x + m.w)));
const h = Math.round(Math.max(...monitors.map((m) => m.y + m.h)));
console.log(`arrangement: ${monitors.length} monitor(s), desktop ${w}x${h}`);

const before = await app.logLines(400);
const changesBefore = saying(before, "arrangement changed").length;
const settledBefore = saying(before, "settled, nothing to bring back").length +
  saying(before, "brought ").length;

// One change is one job and the app debounces a burst, so a message sent less than two
// seconds after the last real change is swallowed — send again rather than reading that as
// an app that never reacted.
let reported = [];
for (let attempt = 1; attempt <= 3 && !reported.length; attempt += 1) {
  const sent = sendDisplayChange(w, h);
  if (!sent.ok) skip(`could not send WM_DISPLAYCHANGE (${sent.stderr || sent.stdout || "no output"})`);
  await app
    .until("the app to report the arrangement change", async () =>
      saying(await app.logLines(400), "arrangement changed")[changesBefore] ?? null,
      { timeout: 4, every: 100 },
    )
    .then((line) => {
      reported = [line];
      console.log(`  sent (attempt ${attempt}): ${line.trim()}`);
    })
    .catch(() => console.log(`  attempt ${attempt}: the app did not report it`));
}
check(reported.length > 0, "the app heard the display change");

// The refit is this report's, not one from an earlier change in the same log.
const refit = (await app.logLines(400))
  .filter((l) => l.includes("the desktop is now"))
  .find((l) => (stamp(l) ?? 0) >= (stamp(reported[0]) ?? 0));
console.log(`  refit:  ${refit?.trim() ?? "never said"}`);
check(!!refit, "the windows and pages were refitted at once");
// The refit is the half that cannot wait: it is what the report is *for*.
if (refit && reported[0]) {
  const gap = (stamp(refit) ?? 0) - (stamp(reported[0]) ?? 0);
  console.log(`  the refit followed the report by ${gap}ms`);
  check(gap >= 0 && gap < 1000, "the refit did not wait for anything");
}

// And then the rearrangement, which does wait. The app says so either way it goes: a
// re-home that brings nothing back used to say nothing at all, which made "rearranged for
// and found whole" read exactly like "never looked at".
let settled = null;
await app
  .until("the desktop to be rearranged for the settled shape", async () => {
    const lines = await app.logLines(400);
    const said = saying(lines, "settled, nothing to bring back")
      .concat(saying(lines, "brought "))
      .filter((l) => (stamp(l) ?? 0) >= (stamp(reported[0]) ?? 0));
    return said[0] ?? null;
  }, { timeout: (SETTLE_MS + TICK_SLACK_MS + 6000) / 1000, every: 200 })
  .then((line) => {
    settled = line;
  })
  .catch(() => null);
console.log(`  settled: ${settled?.trim() ?? "nothing was ever rearranged for"}`);
check(!!settled, "the settled shape was rearranged for");

// The claim in one number: the shape stood still for the settle window first. Pre-fix this
// is a few milliseconds — the report itself is the rearrangement.
if (settled && reported[0]) {
  const wait = (stamp(settled) ?? 0) - (stamp(reported[0]) ?? 0);
  console.log(`  the rearrangement waited ${wait}ms after the report`);
  check(wait >= SETTLE_MS, `waited the settle window, not ${wait}ms`);
  check(wait <= SETTLE_MS + TICK_SLACK_MS + 2000, `and did not wait longer than that`);
}

// The floaties themselves: whatever the log says, nothing may have been moved by this. The
// desktop was already the shape it is in, so a rearrangement over it has no work to do.
const moved = saying(await app.logLines(400), "brought ")
  .filter((l) => (stamp(l) ?? 0) >= (stamp(reported[0]) ?? 0));
if (moved.length) {
  console.log(`  note: the rearrangement moved floaties — ${moved[moved.length - 1].trim()}`);
  console.log("        (legitimate if the desktop really did have something off a screen)");
}

await app.close();
finish();
