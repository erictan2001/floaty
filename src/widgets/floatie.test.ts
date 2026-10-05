/**
 * Unit tests for the one owner of a floatie's place: the gesture rules, and the session
 * that keeps the record and the slot from disagreeing.
 *
 * The three rules under test are pure functions over plain data, which is the point of
 * them: the thresholds, the placement and the release ordering used to be closures inside
 * four different pointerdown handlers, where nothing could reach them. The session tests
 * below use a fake slot in the real `overlaySlots` registry — the same registry the
 * overlay writes — so what they check is what a drag actually does to it.
 *
 * The Tauri imports are stubbed the way lib.test.ts stubs them: the page modules reach for
 * a window handle and an IPC bridge at import time, and these tests only want the maths.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock, winMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  winMock: { label: "desktop-overlay" },
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => undefined),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    // a getter, because `appWin` is created once at import time: a plain property would
    // freeze the label the page started with
    get label() {
      return winMock.label;
    },
    scaleFactor: () => Promise.resolve(1),
    outerPosition: () => Promise.resolve({ x: 0, y: 0 }),
    innerSize: () => Promise.resolve({ width: 1280, height: 800 }),
    setPosition: () => Promise.resolve(),
    setSize: () => Promise.resolve(),
    setSkipTaskbar: () => Promise.resolve(),
  }),
  currentMonitor: () => Promise.resolve(null),
  PhysicalPosition: class {
    constructor(public x: number, public y: number) {}
  },
  PhysicalSize: class {
    constructor(public w: number, public h: number) {}
  },
}));

// The session writes to the page — a hit-rects timer, a click guard, a watcher — and this
// environment has no DOM. These are the globals it reaches for: timers that never fire,
// listeners that are only accepted. No element is created and no style is read in these
// tests, which is why a stub is enough (the same reason lib.test.ts needs no DOM).
vi.stubGlobal("window", {
  setTimeout: () => 0,
  clearTimeout: () => undefined,
  setInterval: () => 0,
  addEventListener: () => undefined,
  removeEventListener: () => undefined,
  dispatchEvent: () => true,
  devicePixelRatio: 1,
  location: { hash: "" },
});
vi.stubGlobal("document", {
  addEventListener: () => undefined,
  removeEventListener: () => undefined,
  hidden: false,
  body: { setPointerCapture: () => undefined, releasePointerCapture: () => undefined },
});

import { overlaySlots, type OverlaySlot, type WidgetRecord } from "./lib";
import {
  dragCrossed,
  floatie,
  placedAt,
  pointerAt,
  resizedTo,
  type DragState,
} from "./floatie";

const rec = (id: string, x = 100, y = 100): WidgetRecord => ({ id, kind: "note", x, y, data: {} });

const slot = (id: string, x = 100, y = 100): OverlaySlot => ({
  id,
  kind: "note",
  x,
  y,
  w: 200,
  h: 120,
  element: { style: {} } as unknown as HTMLElement,
});

/** A press that has not moved yet, holding the floatie 10px right of its corner. */
const press = (grab: { dx: number; dy: number } | null = { dx: 10, dy: 10 }): DragState => ({
  startX: 100,
  startY: 100,
  startSX: 500,
  startSY: 400,
  grab,
  active: false,
  lastX: 500,
  lastY: 400,
});

beforeEach(() => {
  // A session is one per floatie for the page's lifetime, so the registry outlives a test:
  // forgetting the ids here is what makes each test start with no floatie known.
  for (const id of ["a", "b"]) floatie(rec(id)).dispose();
  invokeMock.mockReset();
  overlaySlots.clear();
});

describe("dragCrossed", () => {
  it("stays a click until the pointer has travelled the threshold", () => {
    expect(dragCrossed(press(), { x: 502, y: 402 }, 4)).toBe(false);
  });

  it("counts diagonal travel as travel", () => {
    // 3px in each axis is 4.24px of movement. The overlay drag tested the axes separately,
    // so a press like this one never became a drag however far it went diagonally.
    expect(dragCrossed(press(), { x: 503, y: 403 }, 4)).toBe(true);
  });

  it("stays a drag once it is one, wherever the pointer goes back to", () => {
    const state = press();
    state.active = true;
    expect(dragCrossed(state, { x: 500, y: 400 }, 4)).toBe(true);
  });
});

describe("pointerAt", () => {
  it("reads the css px of an overlay window, which never moves", () => {
    expect(pointerAt({ clientX: 40, clientY: 60, screenX: 900, screenY: 950 })).toEqual({
      x: 40,
      y: 60,
    });
  });

  it("reads the screen px of a window-mode floatie, which moves with the cursor", () => {
    // A window dragged by its content follows the pointer, so clientX stops advancing and
    // the deltas would be near zero. Both appicon and folder carried this rule separately.
    winMock.label = "widget-abc";
    expect(pointerAt({ clientX: 40, clientY: 60, screenX: 900, screenY: 950 })).toEqual({
      x: 900,
      y: 950,
    });
    winMock.label = "desktop-overlay";
  });
});

describe("placedAt", () => {
  it("keeps the grabbed point under the pointer", () => {
    // The pointer is at 900,700 in record space and holds the floatie 10px in from its
    // corner: the floatie goes to 890,690 — not to the pointer, and not by its deltas.
    expect(placedAt(press(), { x: 900, y: 700 }, 400, 300)).toEqual({ x: 890, y: 690 });
  });

  it("falls back to the deltas when there is no screen to map through", () => {
    expect(placedAt(press(), null, 400, 300)).toEqual({ x: 500, y: 400 });
  });

  it("falls back to the deltas when the press has no grab point", () => {
    expect(placedAt(press(null), { x: 900, y: 700 }, 40, 30)).toEqual({ x: 140, y: 130 });
  });

  it("never places from the point the press began at", () => {
    // A release with no coordinates of its own arrives as the last move; a placement that
    // fell back to the press put every floatie back where the drag started.
    const state = press();
    state.lastX = 640;
    state.lastY = 520;
    expect(placedAt(state, null, state.lastX - state.startSX, state.lastY - state.startSY)).toEqual(
      { x: 240, y: 220 },
    );
  });
});

describe("resizedTo", () => {
  const state = { startW: 200, startH: 120, startSX: 500, startSY: 400 };

  it("grows by the corner's travel", () => {
    expect(resizedTo(state, 560, 430, 120, 80)).toEqual({ w: 260, h: 150 });
  });

  it("stops at the minimum, however far the corner is pulled back", () => {
    expect(resizedTo(state, 300, 200, 120, 80)).toEqual({ w: 120, h: 80 });
  });
});

describe("the session", () => {
  it("is one per floatie", () => {
    expect(floatie(rec("a"))).toBe(floatie(rec("a")));
    expect(floatie(rec("a"))).not.toBe(floatie(rec("b")));
  });

  it("moves the record and the slot together, so nothing has to be copied back", () => {
    const record = rec("a");
    const s = slot("a");
    overlaySlots.set("a", s);
    floatie(record).place(640, 480);
    expect([record.x, record.y]).toEqual([640, 480]);
    expect(s.x).toBe(640);
    expect(s.y).toBe(480);
    expect(s.element.style.transform).toBe("translate3d(640px, 480px, 0)");
  });

  it("rounds a placement, so the store and the record cannot disagree by a fraction", () => {
    const record = rec("a");
    overlaySlots.set("a", slot("a"));
    floatie(record).place(640.4, 479.6);
    expect([record.x, record.y]).toEqual([640, 480]);
  });

  it("draws without touching the record, which is what the physics needs", () => {
    const record = rec("a", 100, 100);
    const s = slot("a");
    overlaySlots.set("a", s);
    floatie(record).draw(120.4, 99.7);
    expect([record.x, record.y]).toEqual([100, 100]);
    expect(s.element.style.transform).toBe("translate3d(120.4px, 99.7px, 0)");
  });

  it("draws a stretched slot without recording it as the widget's place", () => {
    // A plugin that covers the desktop to catch the mouse — a drawing surface, a page of
    // save slots — must not move the widget it belongs to.
    const record = rec("a", 100, 100);
    const s = slot("a");
    overlaySlots.set("a", s);
    floatie(record).place(0, 0, { transient: true });
    expect([record.x, record.y]).toEqual([100, 100]);
    expect([s.x, s.y]).toEqual([0, 0]);
  });

  it("knows whether a press is moving it", () => {
    expect(floatie(rec("a")).dragging).toBe(false);
  });

  it("saves what the record says, with no repair", async () => {
    const record = rec("a");
    overlaySlots.set("a", slot("a", 999, 999));
    floatie(record).place(640, 480);
    await floatie(record).commit();
    expect(invokeMock).toHaveBeenCalledWith("floaty_save", { record });
    expect([record.x, record.y]).toEqual([640, 480]);
  });

  it("keeps size in the record's data and on the slot", () => {
    const record = rec("a");
    const s = slot("a");
    overlaySlots.set("a", s);
    floatie(record).size(320, 240);
    expect([record.data["w"], record.data["h"]]).toEqual([320, 240]);
    expect([s.w, s.h]).toEqual([320, 240]);
    expect(s.element.style.width).toBe("320px");
  });

  it("draws a stretched size without recording it as the widget's size", () => {
    // A slot covered over the desktop to catch the mouse is bigger than the widget, and
    // the widget is not bigger. Recording it left the record saying the widget is the
    // size of the monitor, and every restore that read it back made that permanent —
    // the panel came back full-screen and stayed that way. `place` has had this since
    // the first version; `size` did not, so a plugin had to remember two different
    // rules for the same gesture.
    const record = rec("a");
    const s = slot("a");
    overlaySlots.set("a", s);
    floatie(record).size(320, 240);
    floatie(record).size(2560, 1440, undefined, { transient: true });
    expect([record.data["w"], record.data["h"]]).toEqual([320, 240]);
    expect([s.w, s.h]).toEqual([2560, 1440]);
    // and the restore is an ordinary size, which stores the widget's own size again
    floatie(record).size(320, 240);
    expect([s.w, s.h]).toEqual([320, 240]);
  });

  it("writes the newest copy of the record, not the one it was made with", () => {
    // A folder re-reads its record on every change elsewhere; a session left holding the
    // first copy would write a position nothing reads.
    const first = rec("a", 10, 10);
    const session = floatie(first);
    const second = rec("a", 20, 20);
    overlaySlots.set("a", slot("a"));
    floatie(second).place(300, 400);
    expect([second.x, second.y]).toEqual([300, 400]);
    expect([first.x, first.y]).toEqual([10, 10]);
    expect(session.rec).toBe(second);
  });

  it("forgets a floatie that has been disposed", () => {
    const first = floatie(rec("a"));
    first.dispose();
    expect(floatie(rec("a"))).not.toBe(first);
  });
});
