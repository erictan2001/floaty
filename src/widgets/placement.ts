/**
 * `placement` — where a floatie goes.
 *
 * The desktop's geometry used to be restated at every site that needed it: the grid cell in
 * two files and a third that disagreed with both, the icon's body in two more, the collision
 * and edge slops in five, and the screen test in two forms that answered different questions.
 * A policy change was an edit in four files, and none of it could be tested without a window.
 *
 * Here the policy is one module and its answers are data. `plan` takes records and the area
 * to arrange them on and returns where each one goes; `freeSpot` answers for one floatie
 * against the boxes already there; `overlaps`, `onScreen` and `clampTo` are the rules those
 * two are built from; `rippleIndex` is the same cell read as the eye reads a desktop. Nothing
 * here touches a window, a slot or the store — callers apply the plan, and the session owns
 * a record's place — so every rule below is testable as plain numbers.
 *
 * The one thing this module does not decide is how big a *kind* is: that is the manifest's
 * (`pluginSize`), because a plugin declares it. `BODY` is the size of the floatie that has
 * no declaration to read — the desktop icon tile — and is what the modules that draw one use
 * for their own geometry.
 */
import {
  isDesktopItem,
  isPinned,
  layoutPriorityFor,
  pluginSize,
  type Size,
} from "./pluginManifest";
import type { WidgetRecord } from "./lib";

/** A rectangle in desktop space. */
export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A screen, in logical px, as the backend reports it. */
export interface Area {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * The desktop icon tile: what an app, file or folder floatie is drawn at, and the size the
 * modules that draw one need before any record exists. The manifest declares the same numbers
 * for those kinds (`default_size`); this is the client's copy of the tile the grid was
 * measured for.
 */
export const BODY: Size = { w: 92, h: 112 };

/**
 * The arrangement's policy: one cell per icon, the room it keeps from the edges, and the
 * overlap that still counts as clear.
 *
 * `collide` is a slop, not a size: two tiles whose corners overlap by a few px are next to
 * each other, which is how a desktop looks, and treating that as a collision is what moved
 * whole runs of icons. `edge` is the same idea against the screen: a placement keeps this
 * much room from the border so a tile is never flush against it.
 */
export const GRID = {
  /** One icon's cell. */
  w: 100,
  h: 116,
  /** Left and top margin of the first cell. */
  margin: 24,
  /** The strip the grid leaves at the bottom, where resting icons live. */
  bottom: 72,
  /** How far above the screen's bottom edge a resting icon's cell sits. */
  floorGap: 6,
  /** Room kept from the screen's edges. */
  edge: 16,
  /** Overlap that still counts as clear. */
  collide: 12,
};

/**
 * Do two boxes overlap by more than the slop?
 *
 * The one overlap rule: the layout's grid walk, the free-spot search and a new widget's nudge
 * all ask this, and it is the same 12px answer they all used to spell out.
 */
export function overlaps(a: Box, b: Box): boolean {
  const ox = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
  const oy = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
  return ox > GRID.collide && oy > GRID.collide;
}

/**
 * Is this point inside this screen?
 *
 * The half-open rule the backend uses: the right and bottom edges belong to the next screen,
 * so a point on a seam is on exactly one screen and never on two.
 */
export function onScreen(area: Area, x: number, y: number): boolean {
  return x >= area.x && x < area.x + area.w && y >= area.y && y < area.y + area.h;
}

/**
 * The screen a floatie is on, by its x.
 *
 * X only, deliberately: this answers "which floor does a falling icon land on", and a body
 * that hangs over the top or bottom of a screen is still on that screen. A point *test*
 * (`onScreen`) is the other question — whether a window draws a place — and it needs both
 * axes.
 */
export function screenAt(screens: Area[], x: number): Area | undefined {
  return screens.find((m) => x >= m.x && x < m.x + m.w);
}

/**
 * Keep a box inside a screen, `margin` px clear of the border.
 *
 * The default is the arrangement's own `GRID.edge`, which is what a placed floatie
 * wants. The margin is a parameter because the two call sites disagree on purpose: a
 * tile the user drags may sit hard against the border, and passes 0 (see `appicon.ts`),
 * while a folder — which draws a panel and not a tile — keeps the grid's clearance.
 * Both are the same rule with their own margin, not two rules that happened to match.
 */
export function clampTo(area: Area, box: Box, margin = GRID.edge): { x: number; y: number } {
  const minX = area.x + margin;
  const minY = area.y + margin;
  const maxX = Math.max(minX, area.x + area.w - box.w - margin);
  const maxY = Math.max(minY, area.y + area.h - box.h - margin);
  return {
    x: Math.max(minX, Math.min(maxX, box.x)),
    y: Math.max(minY, Math.min(maxY, box.y)),
  };
}

/**
 * Does this record already have a place of its own?
 *
 * A fresh widget the backend writes starts at (0, 0), and (0, 0) is a place nobody
 * chose. The test is "past the origin by more than a hair" rather than "exactly zero",
 * because a widget the user dragged a few px from the corner has been placed: reading
 * that as unplaced is what tidied a hand-dragged position off the desktop and saved the
 * grid cell over it. Ten px is the hair, and the two pages that ask — a layout pass and
 * a widget arriving — used to ask it separately and could drift apart.
 */
export function hasPlace(rec: { x: number; y: number }): boolean {
  return rec.x > 10 || rec.y > 10;
}

/** One floatie to place, and what is already on the desktop. */
export interface SpotRequest {
  /** The floatie being placed — never collides with itself. */
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  /** The screen it is being placed on. */
  area: Area;
  /** The boxes already on the desktop, this floatie's own excluded. */
  others: Box[];
  /** Room a shifted candidate keeps from the box it steps aside for. */
  gap?: number;
}

/**
 * Where a new floatie can go without overlapping what is there.
 *
 * The order is the point: its own place if that is clear, else the nearest place a
 * *neighbour* can be stepped around, else the nearest free grid cell — and its own clamped
 * place if the desktop is full, because a widget with no room still has to exist.
 *
 * Called by the overlay, which is the one page that owns the desktop's arrangement: a
 * floatie's own window has no business moving anything but itself.
 */
export function freeSpot(req: SpotRequest): { x: number; y: number } {
  const { w, h, area } = req;
  const gap = req.gap ?? 8;
  const cur = clampTo(area, req);
  const collides = (x: number, y: number): boolean =>
    req.others.some((o) => overlaps({ x, y, w, h }, o));

  if (!collides(cur.x, cur.y)) return cur;

  const shifted = shiftAround(
    req.others.filter((o) => overlaps({ x: cur.x, y: cur.y, w, h }, o)),
    { ...cur, w, h, gap, area },
    collides,
  );
  if (shifted) return shifted;

  const onGrid = gridAround(cur, { w, h }, area, collides);
  if (onGrid) return onGrid;

  return cur;
}

/** Step aside for one of the boxes in the way, nearest step first. */
function shiftAround(
  colliding: Box[],
  at: { x: number; y: number; w: number; h: number; gap: number; area: Area },
  collides: (x: number, y: number) => boolean,
): { x: number; y: number } | null {
  const { x: curX, y: curY, w, h, gap, area } = at;
  const clamp = (x: number, y: number) => clampTo(area, { x, y, w, h });
  const candidates: Array<{ x: number; y: number; dist: number }> = [];

  for (const o of colliding) {
    const steps = [
      { x: o.x + o.w + gap + 4, y: curY },
      { x: o.x - w - gap - 4, y: curY },
      { x: curX, y: o.y + o.h + gap + 4 },
      { x: curX, y: o.y - h - gap - 4 },
      { x: o.x + o.w + gap + 4, y: o.y },
      { x: o.x - w - gap - 4, y: o.y },
    ];
    for (const step of steps) {
      const s = clamp(step.x, step.y);
      if (!collides(s.x, s.y)) {
        candidates.push({ x: s.x, y: s.y, dist: Math.hypot(s.x - curX, s.y - curY) });
      }
    }
  }
  if (!candidates.length) return null;
  candidates.sort((a, b) => a.dist - b.dist);
  return { x: candidates[0].x, y: candidates[0].y };
}

/** The nearest free cell of the desktop grid, walking outwards from where the floatie is. */
function gridAround(
  cur: { x: number; y: number },
  size: { w: number; h: number },
  area: Area,
  collides: (x: number, y: number) => boolean,
): { x: number; y: number } | null {
  const minX = area.x + GRID.margin;
  const minY = area.y + GRID.margin;
  const baseCol = Math.round((cur.x - minX) / GRID.w);
  const baseRow = Math.round((cur.y - minY) / GRID.h);
  const at = (col: number, row: number) =>
    clampTo(area, { x: minX + col * GRID.w, y: minY + row * GRID.h, ...size });

  for (let ring = 1; ring <= 15; ring++) {
    for (let dc = -ring; dc <= ring; dc++) {
      for (let dr = -ring; dr <= ring; dr++) {
        if (Math.max(Math.abs(dc), Math.abs(dr)) !== ring) continue;
        const p = at(baseCol + dc, baseRow + dr);
        if (!collides(p.x, p.y)) return p;
      }
    }
  }
  return null;
}

/**
 * Where an item sits in the ripple, read the way the eye reads a desktop — left to right,
 * top to bottom. A *place* is what a wave has to be ordered by: the id it used to come from
 * (`num % 6` of `app-17`) scattered the phases randomly across the screen, so even a stagger
 * that applied would not have read as a wave.
 *
 * The cell is the grid's, so a wave lines up with the columns the layout actually uses, and
 * the cell is counted from the *screen* rather than from zero: a second screen's overlay is
 * handed that screen's rectangle, and a point 24px into it is the same first cell the first
 * screen's 24px point is. Counting from the origin alone put every icon on the second
 * monitor a whole screen's worth of columns late in the wave, which is a delay of seconds on
 * a desktop sized like a wave.
 */
export function rippleIndex(at: { x: number; y: number }, area: Area): number {
  const cols = Math.max(1, Math.ceil(area.w / GRID.w));
  const col = Math.max(0, Math.min(cols - 1, Math.round((at.x - area.x) / GRID.w)));
  return Math.max(0, Math.round((at.y - area.y) / GRID.h)) * cols + col;
}

/** What the layout decided: where records go, and which of them were arranged on purpose. */
export interface PlacementPlan {
  /** Records to move, in the order they were placed. */
  moves: Array<{ id: string; x: number; y: number }>;
  /** Records a tidy placed: they carry `data.arranged`, so the next launch leaves them. */
  arranged: string[];
}

/**
 * Where every record that needs a place goes.
 *
 * A saved position is the truth — the user is the one who put it there — so this only finds
 * places for records that have none: a fresh widget the backend wrote at (0, 0), and, when
 * `tidy` says so, the desktop items, which is a deliberate instruction and overrides both a
 * hand-dragged position and a widget's own arrangement. Everything else is taken as placed,
 * and the grid lays itself around it.
 *
 * The desktop has gravity, so a slot is only a place an icon will *stay* if something holds
 * it up: pinned icons fill rows from the top, and a tidy stacks the rest up from the floor,
 * one cell apart.
 *
 * Nothing is written here: the caller applies each move to its record (through the session,
 * which owns a record's place) and marks the arranged ones.
 */
export function plan(
  records: WidgetRecord[],
  opts: { area: Area; top: boolean; tidy?: boolean },
): PlacementPlan {
  const { area, top } = opts;
  const tidy = opts.tidy === true;
  const placed: Array<Box & { id: string }> = [];
  const needsRelocation: WidgetRecord[] = [];
  const floorZone = new Set<string>();
  const moves: Array<{ id: string; x: number; y: number }> = [];
  const arranged: string[] = [];

  const sorted = [...records].sort((a, b) => layoutPriorityFor(a.kind) - layoutPriorityFor(b.kind));

  for (const rec of sorted) {
    // Only this layer's widgets: the other layer's are laid out by its own page, which is the
    // one they are drawn in.
    if (isPinned(rec) !== top) continue;
    const { w, h } = pluginSize(rec);

    if (tidy && isDesktopItem(rec.kind)) {
      if (!isPinned(rec)) floorZone.add(rec.id);
      needsRelocation.push(rec);
      continue;
    }

    // A record a widget placed on purpose is left exactly where it is: the even spacing of a
    // trail reads as a pile of collisions on a curvy path, so the arrangement would not
    // survive a launch without this.
    if (rec.data["arranged"] === true) {
      placed.push({ id: rec.id, x: rec.x, y: rec.y, w, h });
      continue;
    }

    // A saved position is the truth. The test is deliberately not a screen-bounds test:
    // that is how a widget dragged 3px past the edge "lost" its position, failed the test,
    // was tidied onto the grid, and the tidy-up was saved over what the user had chosen.
    if (hasPlace(rec)) {
      placed.push({ id: rec.id, x: rec.x, y: rec.y, w, h });
      continue;
    }
    needsRelocation.push(rec);
  }

  const usableH = Math.max(200, area.h - GRID.margin - GRID.bottom);
  const rowsPerCol = Math.max(1, Math.floor(usableH / GRID.h));
  const floorRows = Math.max(1, Math.floor((area.h - GRID.margin - GRID.floorGap) / GRID.h));
  let gridIndex = 0;
  let floorIndex = 0;

  for (const rec of needsRelocation) {
    const { w, h } = pluginSize(rec);
    const onFloor = tidy && !isPinned(rec);
    let at: { x: number; y: number } | undefined;

    for (let tries = 0; tries < 500 && !at; tries++) {
      const index = onFloor ? floorIndex : gridIndex;
      const rows = onFloor ? floorRows : rowsPerCol;
      const col = Math.floor(index / rows);
      const row = index % rows;
      if (onFloor) floorIndex++;
      else gridIndex++;

      // Clamp into the screen *before* testing for room. The clamp used to run after, so any
      // widget wider than a grid cell (a 240px plugin, the 392px live2d) was pushed back onto
      // the icon grid and ended up hidden behind the tiles that were already there.
      //
      // The grid is measured from the area's own corner, not from zero: a second screen's
      // overlay is handed that screen's rectangle, and placing a widget at x = 24 would put it
      // on the *first* screen while this one drew nothing.
      const box: Box = {
        x: Math.max(
          area.x + GRID.margin,
          Math.min(area.x + GRID.margin + col * GRID.w, area.x + area.w - w - GRID.edge),
        ),
        // from the floor upwards for an icon that rests, from the top down for one that is
        // held: both are measured from the edge the icon answers to
        y: onFloor
          ? Math.max(area.y + GRID.margin, area.y + area.h - GRID.floorGap - h - row * GRID.h)
          : Math.max(
              area.y + GRID.margin,
              Math.min(area.y + GRID.margin + row * GRID.h, area.y + area.h - h - GRID.edge),
            ),
        w,
        h,
      };
      // A stack of resting icons is not a collision: the floor zone is what the physics does
      // with a pile, and tidy is only choosing the columns.
      const collides = placed.some((p) => overlaps(box, p) && !floorZone.has(p.id));
      if (!collides) at = { x: box.x, y: box.y };
    }

    // A full desktop still gets the widget: the clamped spot rather than an endless search.
    // (The expression this replaced could only ever answer the margin corner, whatever the
    // screen and the widget's size.)
    at ??= clampTo(area, { x: area.x + GRID.margin, y: area.y + GRID.margin, w, h });

    moves.push({ id: rec.id, x: at.x, y: at.y });
    placed.push({ id: rec.id, x: at.x, y: at.y, w, h });
    if (tidy && isDesktopItem(rec.kind)) arranged.push(rec.id);
  }

  return { moves, arranged };
}
