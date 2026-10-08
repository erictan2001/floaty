/**
 * Unit tests for the placement policy: where a floatie goes, and why.
 *
 * Every rule under test is arithmetic over plain data, which is the point of the module —
 * these tests stub nothing. There is no Tauri mock, no `window`, no slot registry: the
 * module answers from numbers in and numbers out, so a test here is a policy statement
 * rather than a simulation of a window that does not exist.
 *
 * The one seam is `pluginManifest`, which holds the backend's table of kinds: how big a
 * kind is, where it sorts, and whether it stands for something on disk. It is stubbed
 * below with the built-in table from `src-tauri/src/plugins.rs` — the same facts the real
 * backend serves — so a test can say "a note is 300x330" and mean it. Nothing else is
 * replaced, and no DOM global is touched anywhere in this file.
 */
import { describe, expect, it, vi } from "vitest";
import type { WidgetRecord } from "./lib";

/** The built-in kinds, as `src-tauri/src/plugins.rs` declares them. */
const MANIFEST: Record<string, { size: [number, number]; priority: number; desktop: boolean }> = {
  note: { size: [300, 330], priority: 2, desktop: false },
  clock: { size: [250, 330], priority: 3, desktop: false },
  app: { size: [92, 112], priority: 10, desktop: true },
  file: { size: [92, 112], priority: 10, desktop: true },
  folder: { size: [92, 112], priority: 10, desktop: true },
};

vi.mock("./pluginManifest", async (importOriginal) => {
  const real = (await importOriginal()) as Record<string, unknown>;
  return {
    ...real,
    isDesktopItem: (kind: string) => MANIFEST[kind]?.desktop === true,
    layoutPriorityFor: (kind: string) => MANIFEST[kind]?.priority ?? 10,
    pluginSize: (rec: WidgetRecord) => {
      const base = MANIFEST[rec.kind]?.size ?? [100, 100];
      return {
        w: (rec.data["w"] as number) ?? base[0],
        h: (rec.data["h"] as number) ?? base[1],
      };
    },
  };
});

const { BODY, GRID, clampTo, freeSpot, hasPlace, onScreen, overlaps, plan, rippleIndex, screenAt } =
  await import("./placement");

/** The 200% screen: 1440x960 logical, at the origin. */
const SCREEN_A = { x: 0, y: 0, w: 1440, h: 960 };
/** The 150% screen beside it: 1280x720 logical, 1440px to the right. */
const SCREEN_B = { x: 1440, y: 0, w: 1280, h: 720 };

/** A record the way the backend writes one; no import that would drag in a page module. */
function rec(
  id: string,
  kind: string,
  x: number,
  y: number,
  data: Record<string, unknown> = {},
): WidgetRecord {
  return { id, kind, x, y, data };
}

describe("BODY and GRID", () => {
  it("is the icon tile the manifest declares for the desktop-item kinds", () => {
    // BODY is this module's own copy of the tile the grid was measured for, so it has to
    // agree with what the backend says an app/file/folder floatie is.
    expect(BODY).toEqual({ w: 92, h: 112 });
    expect(MANIFEST["app"]!.size).toEqual([BODY.w, BODY.h]);
    expect(MANIFEST["file"]!.size).toEqual([BODY.w, BODY.h]);
    expect(MANIFEST["folder"]!.size).toEqual([BODY.w, BODY.h]);
  });

  it("keeps the arrangement's own numbers", () => {
    expect(GRID).toEqual({
      w: 100,
      h: 116,
      margin: 24,
      bottom: 72,
      floorGap: 6,
      edge: 16,
      collide: 12,
    });
  });

  it("has a cell that holds the tile it was measured for", () => {
    expect(GRID.w).toBeGreaterThanOrEqual(BODY.w);
    expect(GRID.h).toBeGreaterThanOrEqual(BODY.h);
  });
});

describe("overlaps", () => {
  const a = { x: 0, y: 0, w: 100, h: 100 };

  it("is false for boxes that only touch", () => {
    expect(overlaps(a, { x: 100, y: 0, w: 100, h: 100 })).toBe(false);
    expect(overlaps(a, { x: 0, y: 100, w: 100, h: 100 })).toBe(false);
    expect(overlaps(a, { x: 100, y: 100, w: 100, h: 100 })).toBe(false);
    expect(overlaps(a, { x: -100, y: 0, w: 100, h: 100 })).toBe(false);
  });

  it("is false at exactly the slop on either axis: the slop is a slop, not a size", () => {
    // 100 wide against a box starting at 100: they share 12px of x, and the rule is
    // "more than 12", so 12 is still neighbours and 13 is a collision.
    expect(overlaps(a, { x: 100, y: 0, w: 100, h: 100 })).toBe(false);
    expect(overlaps({ x: 0, y: 0, w: 112, h: 100 }, { x: 100, y: 0, w: 100, h: 100 })).toBe(false);
    expect(overlaps({ x: 0, y: 0, w: 150, h: 100 }, { x: 100, y: 88, w: 100, h: 100 })).toBe(false);
  });

  it("is true past the slop on both axes at once", () => {
    expect(overlaps({ x: 0, y: 0, w: 113, h: 100 }, { x: 100, y: 0, w: 100, h: 100 })).toBe(true);
    expect(overlaps({ x: 0, y: 0, w: 150, h: 100 }, { x: 100, y: 87, w: 100, h: 100 })).toBe(true);
    expect(overlaps({ x: 0, y: 0, w: 100, h: 100 }, { x: 40, y: 40, w: 100, h: 100 })).toBe(true);
  });

  it("is false when one axis overlaps deeply and the other is inside the slop", () => {
    // 50px of x but only 12px of y: neighbours, so a run of icons is not a collision.
    expect(overlaps({ x: 0, y: 0, w: 150, h: 100 }, { x: 100, y: 88, w: 100, h: 100 })).toBe(false);
    // 50px of y but only 12px of x
    expect(overlaps({ x: 0, y: 0, w: 100, h: 150 }, { x: 88, y: 100, w: 100, h: 100 })).toBe(false);
  });

  it("is true of a box against itself", () => {
    expect(overlaps(a, { ...a })).toBe(true);
  });
});

describe("onScreen", () => {
  it("claims the whole screen but its far edges", () => {
    expect(onScreen(SCREEN_A, 0, 0)).toBe(true);
    expect(onScreen(SCREEN_A, 1439, 959)).toBe(true);
  });

  it("gives the right and bottom seams to the next screen, not this one", () => {
    // Half-open: a point exactly on the seam is on one screen and never two.
    expect(onScreen(SCREEN_A, 1440, 100)).toBe(false);
    expect(onScreen(SCREEN_A, 100, 960)).toBe(false);
    expect(onScreen(SCREEN_A, 1440, 960)).toBe(false);

    expect(onScreen(SCREEN_B, 1440, 0)).toBe(true);
    expect(onScreen(SCREEN_B, 2719, 719)).toBe(true);
    expect(onScreen(SCREEN_B, 2720, 100)).toBe(false); // past its right edge
    expect(onScreen(SCREEN_B, 100, 720)).toBe(false); // its bottom seam is nobody's
  });

  it("is false off every edge of the screen", () => {
    expect(onScreen(SCREEN_A, -1, 100)).toBe(false);
    expect(onScreen(SCREEN_A, 100, -1)).toBe(false);
  });
});

describe("screenAt", () => {
  const screens = [SCREEN_A, SCREEN_B];

  it("picks the screen whose horizontal band contains x", () => {
    expect(screenAt(screens, 0)).toBe(SCREEN_A);
    expect(screenAt(screens, 700)).toBe(SCREEN_A);
    expect(screenAt(screens, 1439)).toBe(SCREEN_A);
    expect(screenAt(screens, 1440)).toBe(SCREEN_B); // the seam belongs to the right screen
    expect(screenAt(screens, 2719)).toBe(SCREEN_B);
  });

  it("is undefined off the arrangement, since x is the only question", () => {
    expect(screenAt(screens, 2720)).toBeUndefined();
    expect(screenAt(screens, -10)).toBeUndefined();
    expect(screenAt([], 100)).toBeUndefined();
  });

  it("does not care about y: a body hanging off the top or bottom is still on that screen", () => {
    // `screenAt` is which floor a falling icon lands on, so it answers by x alone. The
    // point test (`onScreen`) is the other question and does need both axes.
    expect(screenAt(screens, 500)).toBe(SCREEN_A);
    expect(onScreen(SCREEN_A, 500, -400)).toBe(false);
    expect(screenAt(screens, 2000)).toBe(SCREEN_B);
    expect(onScreen(SCREEN_B, 2000, 2000)).toBe(false);
  });
});

describe("clampTo", () => {
  it("passes a box that is already inside through unchanged", () => {
    expect(clampTo(SCREEN_A, { x: 700, y: 700, w: 100, h: 100 })).toEqual({ x: 700, y: 700 });
  });

  it("keeps GRID.edge clear of every border", () => {
    const r = clampTo(SCREEN_A, { x: 5000, y: 200, w: 120, h: 120 });
    expect(r.x).toBe(SCREEN_A.x + SCREEN_A.w - 120 - GRID.edge);
    expect(r.x + 120).toBeLessThanOrEqual(SCREEN_A.x + SCREEN_A.w);
    expect(r.x).toBeGreaterThanOrEqual(SCREEN_A.x + GRID.edge);
    expect(r.y).toBe(200);

    expect(clampTo(SCREEN_A, { x: 200, y: 5000, w: 120, h: 120 })).toEqual({
      x: 200,
      y: SCREEN_A.y + SCREEN_A.h - 120 - GRID.edge,
    });

    expect(clampTo(SCREEN_A, { x: -400, y: -400, w: 120, h: 120 })).toEqual({
      x: GRID.edge,
      y: GRID.edge,
    });
  });

  it("confines to the screen it was given, not the arrangement", () => {
    // x 5000 is far right of both screens; the answer is still on screen B.
    const r = clampTo(SCREEN_B, { x: 5000, y: 100, w: 120, h: 120 });
    expect(r.x).toBe(SCREEN_B.x + SCREEN_B.w - 120 - GRID.edge);
    expect(r.x).toBeGreaterThanOrEqual(SCREEN_B.x + GRID.edge);
    // never left of the second screen, and never past its right edge
    expect(r.x).toBeGreaterThan(SCREEN_A.x + SCREEN_A.w);
    expect(r.x + 120).toBeLessThanOrEqual(SCREEN_B.x + SCREEN_B.w);
  });

  it("gives a box bigger than the screen the minimum, never a negative or inverted range", () => {
    // The usable range here is negative on both axes. The answer is the minimum (edge),
    // not min clamped through a max that is below it — which is the inverted-range case.
    expect(clampTo(SCREEN_A, { x: 100, y: 100, w: 4000, h: 3000 })).toEqual({
      x: GRID.edge,
      y: GRID.edge,
    });
    // Exactly the screen's own size, edge on all four sides: still the minimum.
    expect(clampTo(SCREEN_A, { x: 100, y: 100, w: SCREEN_A.w, h: SCREEN_A.h })).toEqual({
      x: GRID.edge,
      y: GRID.edge,
    });
    // A tiny screen where the box is bigger than the screen itself.
    expect(clampTo({ x: 0, y: 0, w: 100, h: 100 }, { x: 0, y: 0, w: 400, h: 300 })).toEqual({
      x: GRID.edge,
      y: GRID.edge,
    });
  });

  it("never returns a number below the screen's own origin", () => {
    for (const area of [SCREEN_A, SCREEN_B, { x: 0, y: 0, w: 300, h: 200 }]) {
      for (const box of [
        { x: -9999, y: -9999, w: 10, h: 10 },
        { x: 9999, y: 9999, w: 10, h: 10 },
        { x: 0, y: 0, w: 99999, h: 99999 },
      ]) {
        const r = clampTo(area, box);
        expect(r.x, `x for ${JSON.stringify(box)} on ${area.x}`).toBeGreaterThanOrEqual(area.x);
        expect(r.y, `y for ${JSON.stringify(box)} on ${area.y}`).toBeGreaterThanOrEqual(area.y);
      }
    }
  });

  it("takes the margin as an argument, and the default is the grid's own", () => {
    // The margin is a parameter because the two callers disagree on purpose: a dragged
    // app tile sits flush against the border (margin 0, `appicon.ts`), while a folder and
    // the item dragged out of one keep the grid's clearance (`folder.ts`).
    expect(clampTo(SCREEN_A, { x: -500, y: -500, w: 100, h: 100 }, 0)).toEqual({ x: 0, y: 0 });
    expect(clampTo(SCREEN_A, { x: -500, y: -500, w: 100, h: 100 })).toEqual({
      x: GRID.edge,
      y: GRID.edge,
    });
    // the same two margins on the far side, and on a screen that is not at the origin
    const box = { x: 9999, y: 9999, w: 100, h: 100 };
    expect(clampTo(SCREEN_B, box, 0)).toEqual({
      x: SCREEN_B.x + SCREEN_B.w - 100,
      y: SCREEN_B.y + SCREEN_B.h - 100,
    });
    expect(clampTo(SCREEN_B, box)).toEqual({
      x: SCREEN_B.x + SCREEN_B.w - 100 - GRID.edge,
      y: SCREEN_B.y + SCREEN_B.h - 100 - GRID.edge,
    });
    // a margin wider than the screen still answers the minimum rather than inverting
    expect(clampTo(SCREEN_A, { x: 0, y: 0, w: 100, h: 100 }, 9999)).toEqual({
      x: SCREEN_A.x + 9999,
      y: SCREEN_A.y + 9999,
    });
  });
});

describe("freeSpot", () => {
  it("returns a clear place unchanged", () => {
    expect(
      freeSpot({
        id: "n",
        x: 300,
        y: 500,
        w: 100,
        h: 100,
        area: SCREEN_A,
        others: [{ x: 900, y: 700, w: 50, h: 50 }],
      }),
    ).toEqual({ x: 300, y: 500 });
  });

  it("clamps a clear place that was asked for off the screen", () => {
    expect(
      freeSpot({ id: "n", x: 1900, y: 2000, w: 100, h: 100, area: SCREEN_A, others: [] }),
    ).toEqual({
      x: SCREEN_A.x + SCREEN_A.w - 100 - GRID.edge,
      y: SCREEN_A.y + SCREEN_A.h - 100 - GRID.edge,
    });
  });

  it("steps aside for the box it would land on, and lands beside it", () => {
    const obstacle = { x: 300, y: 100, w: 150, h: 150 };
    const r = freeSpot({
      id: "n",
      x: 400,
      y: 100,
      w: 100,
      h: 100,
      area: SCREEN_A,
      others: [obstacle],
    });
    expect(overlaps({ ...r, w: 100, h: 100 }, obstacle)).toBe(false);
    // a step to the right, on the obstacle's own row, no further than the search reaches
    expect(r.y).toBe(100);
    expect(r.x).toBeGreaterThan(obstacle.x + obstacle.w);
    expect(Math.hypot(r.x - 400, r.y - 100)).toBeLessThan(200);
  });

  it("keeps the requested gap from the box it stepped around", () => {
    const obstacle = { x: 300, y: 100, w: 150, h: 150 };
    const withGap = freeSpot({
      id: "n",
      x: 400,
      y: 100,
      w: 100,
      h: 100,
      area: SCREEN_A,
      others: [obstacle],
      gap: 40,
    });
    const withoutGap = freeSpot({
      id: "n",
      x: 400,
      y: 100,
      w: 100,
      h: 100,
      area: SCREEN_A,
      others: [obstacle],
      gap: 0,
    });
    // a bigger gap is never a nearer answer than a smaller one on the same side
    expect(withGap.x).toBeGreaterThan(withoutGap.x);
    expect(withoutGap.x - (obstacle.x + obstacle.w)).toBeGreaterThanOrEqual(0);
  });

  it("falls back to a free grid cell when the surroundings are full", () => {
    // A wall of boxes on both sides of the asked-for place, so every step aside is
    // blocked and the grid walk has to find somewhere else.
    const others = [
      { x: 0, y: 0, w: 700, h: 100 },
      { x: 0, y: 100, w: 700, h: 100 },
      { x: 0, y: 200, w: 700, h: 100 },
      { x: 712, y: 0, w: 100, h: 200 },
    ];
    const r = freeSpot({ id: "n", x: 24, y: 24, w: 100, h: 100, area: SCREEN_A, others });
    for (const o of others)
      expect(overlaps({ ...r, w: 100, h: 100 }, o), `clear of ${o.x},${o.y}`).toBe(false);
    // and the answer is a cell of the grid, edge-clear of the screen it was asked for
    expect(r.x).toBe(SCREEN_A.x + GRID.edge);
    expect(r.x - (SCREEN_A.x + GRID.margin)).toBeLessThanOrEqual(GRID.w);
  });

  it("still returns a clamped place on a desktop with no room at all", () => {
    // Nothing is free, so the answer is the requested place pulled inside the screen —
    // overlapping rather than missing, because a widget with no room still has to exist.
    const area = { x: 0, y: 0, w: 200, h: 200 };
    const r = freeSpot({
      id: "n",
      x: 500,
      y: 500,
      w: 100,
      h: 100,
      area,
      others: [{ x: 0, y: 0, w: 200, h: 200 }],
    });
    expect(r).toEqual(clampTo(area, { x: 500, y: 500, w: 100, h: 100 }));
    expect(r).toEqual({ x: 84, y: 84 });
  });

  it("never gives an answer off the screen it was asked for", () => {
    const area = { x: 0, y: 0, w: 150, h: 150 };
    const r = freeSpot({
      id: "n",
      x: 500,
      y: 500,
      w: 100,
      h: 100,
      area,
      others: [{ x: 0, y: 0, w: 150, h: 150 }],
    });
    expect(r.x).toBeGreaterThanOrEqual(area.x);
    expect(r.y).toBeGreaterThanOrEqual(area.y);
    expect(r.x).toBeLessThan(area.x + area.w);
    expect(r.y).toBeLessThan(area.y + area.h);
  });
});

describe("plan", () => {
  it("leaves a record with a saved position exactly where it is", () => {
    const r = plan([rec("s", "note", 300, 400)], { area: SCREEN_A, top: false });
    expect(r.moves).toEqual([]);
    expect(r.arranged).toEqual([]);
  });

  it("takes either axis as a saved position", () => {
    // The rule is "x > 10 OR y > 10", so a widget dragged clear of the corner counts.
    expect(plan([rec("a", "note", 500, 0)], { area: SCREEN_A, top: false }).moves).toEqual([]);
    expect(plan([rec("b", "note", 0, 500)], { area: SCREEN_A, top: false }).moves).toEqual([]);
    // and the fresh-widget corner is the one that has no position
    expect(plan([rec("c", "note", 10, 10)], { area: SCREEN_A, top: false }).moves).toHaveLength(1);
  });

  it("gives a record at (0, 0) a place on the grid", () => {
    const r = plan([rec("z", "note", 0, 0)], { area: SCREEN_A, top: false });
    expect(r.moves).toEqual([{ id: "z", x: GRID.margin, y: GRID.margin }]);
  });

  it("keeps every placed record out of the moves", () => {
    const r = plan(
      [rec("a", "note", 300, 400), rec("b", "note", 0, 0), rec("c", "note", 700, 100)],
      { area: SCREEN_A, top: false },
    );
    expect(r.moves.map((m) => m.id)).toEqual(["b"]);
  });

  it("lays new widgets out around the ones already placed", () => {
    // The first new widget takes the top-left cell; the second must not land on it.
    const r = plan([rec("a", "note", 0, 0), rec("b", "note", 0, 0)], {
      area: SCREEN_A,
      top: false,
    });
    expect(r.moves).toHaveLength(2);
    expect(overlaps({ ...r.moves[0]!, w: 300, h: 330 }, { ...r.moves[1]!, w: 300, h: 330 })).toBe(
      false,
    );
  });

  it("plans only the records on the layer it was asked for", () => {
    const desktopItem = rec("d", "app", 0, 0);
    const topWidget = rec("t", "note", 0, 0, { on_top: true });

    expect(
      plan([desktopItem, topWidget], { area: SCREEN_A, top: false }).moves.map((m) => m.id),
    ).toEqual(["d"]);
    expect(
      plan([desktopItem, topWidget], { area: SCREEN_A, top: true }).moves.map((m) => m.id),
    ).toEqual(["t"]);
  });

  it("reads the layer off the record, not off the kind", () => {
    // A pinned desktop item belongs to the top layer, and a desktop-layer note to this one.
    const pinnedApp = rec("p", "app", 0, 0, { on_top: true });
    expect(plan([pinnedApp], { area: SCREEN_A, top: false }).moves).toEqual([]);
    expect(plan([pinnedApp], { area: SCREEN_A, top: true }).moves).toHaveLength(1);
  });

  it("relocates the desktop-item kinds on a tidy and reports them in arranged", () => {
    for (const kind of ["app", "file", "folder"]) {
      const r = plan([rec("i", kind, 700, 500)], { area: SCREEN_A, top: false, tidy: true });
      expect(r.moves, `kind ${kind}`).toHaveLength(1);
      expect(r.arranged, `kind ${kind}`).toEqual(["i"]);
      // a tidy overrides a hand-dragged position, which is the point of the instruction
      expect(r.moves[0]).not.toMatchObject({ x: 700, y: 500 });
    }
  });

  it("leaves a kind that is not a desktop item alone on a tidy", () => {
    const r = plan([rec("n", "note", 300, 400), rec("c", "clock", 800, 200)], {
      area: SCREEN_A,
      top: false,
      tidy: true,
    });
    expect(r.moves).toEqual([]);
    expect(r.arranged).toEqual([]);
  });

  it("stacks a tidied icon from the floor upwards, one cell apart", () => {
    const r = plan([rec("a", "app", 0, 0), rec("f", "file", 0, 0), rec("d", "folder", 0, 0)], {
      area: SCREEN_A,
      top: false,
      tidy: true,
    });
    expect(r.moves.map((m) => m.id)).toEqual(["a", "f", "d"]);
    expect(r.arranged).toEqual(["a", "f", "d"]);

    const ys = r.moves.map((m) => m.y);
    // the first icon rests on the floor, the rest stack up from it
    expect(ys[0]).toBe(SCREEN_A.h - GRID.floorGap - BODY.h);
    for (let i = 1; i < ys.length; i++) {
      expect(ys[i - 1]! - ys[i]!).toBe(GRID.h); // each one exactly one cell higher
    }
    // and the whole stack stays inside the screen, and clear of the top edge
    expect(Math.min(...ys)).toBeGreaterThanOrEqual(GRID.margin);
  });

  it("fills columns up the floor before starting the next one", () => {
    // More icons than fit in one column of the floor zone: the surplus moves right.
    const r = plan(
      Array.from({ length: 10 }, (_, i) => rec(`i${i}`, "app", 0, 0)),
      { area: SCREEN_A, top: false, tidy: true },
    );
    const xs = [...new Set(r.moves.map((m) => m.x))];
    expect(xs.length).toBeGreaterThan(1);
    // same column, same x, strictly rising y
    const first = r.moves.filter((m) => m.x === xs[0]).map((m) => m.y);
    for (let i = 1; i < first.length; i++) expect(first[i - 1]! - first[i]!).toBe(GRID.h);
  });

  it("lays a pinned icon from the top down instead, and still counts it as arranged", () => {
    const r = plan(
      [rec("a", "app", 0, 0, { on_top: true }), rec("b", "app", 0, 0, { on_top: true })],
      {
        area: SCREEN_A,
        top: true,
        tidy: true,
      },
    );
    expect(r.arranged).toEqual(["a", "b"]);
    expect(r.moves[0]!.y).toBe(GRID.margin);
    expect(r.moves[1]!.y).toBe(GRID.margin + GRID.h);
  });

  it("returns nothing in arranged on a pass that is not a tidy", () => {
    const r = plan([rec("a", "app", 0, 0), rec("b", "file", 0, 0), rec("n", "note", 0, 0)], {
      area: SCREEN_A,
      top: false,
    });
    expect(r.arranged).toEqual([]);
    // the desktop items are placed on the grid, from the top, not stacked on the floor.
    // A note is 330 tall against a 116 cell, so the icon beside it drops a whole row.
    expect(r.moves.map((m) => [m.id, m.x, m.y])).toEqual([
      ["n", GRID.margin, GRID.margin],
      ["a", GRID.margin, GRID.margin + 3 * GRID.h],
      ["b", GRID.margin, GRID.margin + 4 * GRID.h],
    ]);
    expect(r.moves.every((m) => m.y < SCREEN_A.h - GRID.bottom)).toBe(true); // not on the floor
  });

  it("still places a record whose kind this build does not know", () => {
    // The fallback size, and a place on the grid: an unknown kind is not dropped.
    const r = plan([rec("u", "no-such-kind", 0, 0)], { area: SCREEN_A, top: false });
    expect(r.moves).toEqual([{ id: "u", x: GRID.margin, y: GRID.margin }]);
  });

  it("places an unknown kind around a real one at its fallback size", () => {
    // A saved app (92 wide) at x 110 leaves room for the 100px fallback but not for a
    // 300px note, so the unknown kind is the one that has to step down a row.
    const r = plan([rec("s", "app", 110, 24), rec("u", "no-such-kind", 0, 0)], {
      area: SCREEN_A,
      top: false,
    });
    expect(r.moves).toEqual([{ id: "u", x: GRID.margin, y: GRID.margin + GRID.h }]);
  });

  it("honours data.arranged: a widget's own placement survives a pass", () => {
    const r = plan([rec("w", "note", 640, 480, { arranged: true })], {
      area: SCREEN_A,
      top: false,
    });
    expect(r.moves).toEqual([]);
  });

  it("never moves two records onto the same place", () => {
    const records = [
      rec("saved", "note", 400, 300),
      ...Array.from({ length: 5 }, (_, i) => rec(`new${i}`, "note", 0, 0)),
    ];
    const r = plan(records, { area: SCREEN_A, top: false });
    expect(r.moves).toHaveLength(5);
    const boxes = r.moves.map((m) => ({ ...m, w: 300, h: 330 }));
    for (let i = 0; i < boxes.length; i++) {
      for (let j = i + 1; j < boxes.length; j++) {
        expect(overlaps(boxes[i]!, boxes[j]!), `${boxes[i]!.id} vs ${boxes[j]!.id}`).toBe(false);
      }
    }
  });

  it("measures the grid from the area's own corner, not from zero", () => {
    // A second screen's overlay is handed that screen's rectangle. Placing a fresh record at
    // x = 24 would put it on the *first* screen while this one drew nothing.
    const r = plan([rec("n", "app", 0, 0)], { area: SCREEN_B, top: false });
    expect(r.moves).toEqual([
      { id: "n", x: SCREEN_B.x + GRID.margin, y: SCREEN_B.y + GRID.margin },
    ]);
    const at = r.moves[0]!;
    expect(onScreen(SCREEN_B, at.x, at.y)).toBe(true);
    expect(onScreen(SCREEN_A, at.x, at.y)).toBe(false);
  });

  it("stacks a tidied icon from the second screen's own floor", () => {
    const r = plan([rec("a", "app", 0, 0), rec("f", "file", 0, 0)], {
      area: SCREEN_B,
      top: false,
      tidy: true,
    });
    expect(r.moves[0]!.y).toBe(SCREEN_B.y + SCREEN_B.h - GRID.floorGap - BODY.h);
    expect(r.moves[0]!.x).toBe(SCREEN_B.x + GRID.margin);
    for (const m of r.moves) expect(onScreen(SCREEN_B, m.x, m.y)).toBe(true);
  });

  it("gives a full desktop the clamped spot, whatever the widget's size", () => {
    // Two 104px-wide records on a 120px screen: the first still finds the grid's margin
    // corner, the second collides with it on every one of the 500 tries and falls back to a
    // clamp — inside the screen and clear of its edge, not a fixed corner that ignores both
    // the screen and the box.
    const tiny = { x: 0, y: 0, w: 120, h: 90 };
    const box = { w: 104, h: 74 };
    const r = plan([rec("w", "note", 0, 0, box), rec("v", "note", 0, 0, box)], {
      area: tiny,
      top: false,
    });
    expect(r.moves).toEqual([
      { id: "w", x: GRID.margin, y: GRID.margin },
      { id: "v", x: tiny.x + GRID.edge, y: tiny.y + GRID.edge },
    ]);
    // The fallback is a clamp, so it is inside the screen it was asked for. (The first
    // record's place is the grid's margin — the floor that stops a wide widget being pushed
    // left of it — and on a screen this small that is simply as far left as it can go.)
    const [w, v] = r.moves;
    expect(v!.x + box.w).toBeLessThanOrEqual(tiny.x + tiny.w);
    expect(v!.y + box.h).toBeLessThanOrEqual(tiny.y + tiny.h);
    expect(w!.x).toBeGreaterThanOrEqual(GRID.margin);
  });
});

describe("rippleIndex", () => {
  it("orders a row left to right", () => {
    // 1440 / 100 is 15 columns, so the first three cells of the top row are 0, 1, 2.
    expect(rippleIndex({ x: 24, y: 24 }, SCREEN_A)).toBe(0);
    expect(rippleIndex({ x: 124, y: 24 }, SCREEN_A)).toBe(1);
    expect(rippleIndex({ x: 224, y: 24 }, SCREEN_A)).toBe(2);
  });

  it("puts the second row after the whole first row", () => {
    // a cell in the first row, and the first cell of the second row
    expect(rippleIndex({ x: 1424, y: 24 }, SCREEN_A)).toBe(14);
    expect(rippleIndex({ x: 24, y: 140 }, SCREEN_A)).toBe(15);
    // and the last cell of the second row is the last index of that row
    expect(rippleIndex({ x: 1424, y: 140 }, SCREEN_A)).toBe(29);
  });

  it("reads the row off y in grid cells", () => {
    // three rows down, eight columns in: 2 * 15 + 8
    expect(rippleIndex({ x: 824, y: 372 }, SCREEN_A)).toBe(53);
  });

  it("uses the width it is given for the column count", () => {
    // 400 / 100 is 4 columns, so the second row starts at 4, not at 15
    const narrow = { x: 0, y: 0, w: 400, h: 960 };
    expect(rippleIndex({ x: 24, y: 24 }, narrow)).toBe(0);
    expect(rippleIndex({ x: 224, y: 24 }, narrow)).toBe(2);
    expect(rippleIndex({ x: 24, y: 140 }, narrow)).toBe(4);
  });

  it("stays in range for a place off the grid, and for a width with no room", () => {
    expect(rippleIndex({ x: 99999, y: 24 }, SCREEN_A)).toBe(14); // clamped to the last column
    expect(rippleIndex({ x: 24, y: -500 }, SCREEN_A)).toBe(0); // never negative
    expect(rippleIndex({ x: 24, y: 24 }, { x: 0, y: 0, w: 0, h: 0 })).toBe(0); // at least one column
  });

  it("counts cells from the screen's own corner, so a second monitor reads the same", () => {
    // 24px into screen B is the first cell, and 24px into screen A is the first cell:
    // same place on the desktop, so the same place in the wave. Counting from zero made
    // every icon on the second monitor a whole screen of columns late.
    expect(rippleIndex({ x: SCREEN_B.x + 24, y: SCREEN_B.y + 24 }, SCREEN_B)).toBe(
      rippleIndex({ x: 24, y: 24 }, SCREEN_A),
    );
    expect(rippleIndex({ x: SCREEN_B.x + 24, y: SCREEN_B.y + 24 }, SCREEN_B)).toBe(0);
    expect(rippleIndex({ x: SCREEN_B.x + 124, y: SCREEN_B.y + 24 }, SCREEN_B)).toBe(1);
    // 1280 / 100 is 13 columns, so the first cell of the second row is 13 — the row is
    // counted off this screen's width, which is what the old width argument was for.
    expect(rippleIndex({ x: SCREEN_B.x + 24, y: SCREEN_B.y + 140 }, SCREEN_B)).toBe(13);
  });

  it("clamps to the last column of the screen it was given, not the last of a wider one", () => {
    // 1280 / 100 is 13 columns, so the last cell of screen B's top row is 12 — asking for
    // the origin-screen's 15 columns here would put a tile a full row off the wave.
    expect(rippleIndex({ x: 99999, y: SCREEN_B.y + 24 }, SCREEN_B)).toBe(12);
  });
});

describe("hasPlace", () => {
  it("is false for the corner a fresh record is written at", () => {
    // The backend writes a new widget at (0, 0), and that is not a place anyone chose.
    expect(hasPlace({ x: 0, y: 0 })).toBe(false);
  });

  it("is true for anything past the ten pixels by the origin", () => {
    // Either axis counts: a widget dragged against the left edge has still been placed,
    // and reading that as unplaced is what tidied a hand-dragged position over.
    expect(hasPlace({ x: 11, y: 0 })).toBe(true);
    expect(hasPlace({ x: 0, y: 11 })).toBe(true);
  });

  it("treats the first ten pixels as the corner, not as a position", () => {
    // Not a bounds test: a widget 3px past the screen's edge keeps its place too, which
    // is the whole reason this is "past the origin" rather than "inside the screen".
    expect(hasPlace({ x: 4, y: 4 })).toBe(false);
    expect(hasPlace({ x: 10, y: 0 })).toBe(false);
  });

  it("is what plan reads, so a record it calls placed is not moved", () => {
    // The same question in both places: plan leaves it alone, and the predicate says so.
    const saved = rec("s", "note", 4, 400);
    expect(hasPlace(saved)).toBe(true);
    expect(plan([saved], { area: SCREEN_A, top: false }).moves).toEqual([]);

    const fresh = rec("f", "note", 0, 0);
    expect(hasPlace(fresh)).toBe(false);
    expect(plan([fresh], { area: SCREEN_A, top: false }).moves).toHaveLength(1);
  });
});
