/**
 * Unit tests for the pure logic in lib.ts: the screen/record coordinate mapping,
 * the placement and confinement maths, and the small calculation helpers.
 *
 * lib.ts is a Tauri page module, so the only two things it reaches for at import
 * time — the current window handle and the IPC bridge — are stubbed here. Nothing
 * in this file touches the DOM: every function under test is a calculation over
 * data the backend supplies, and the screen fixtures below are the arrangement the
 * app actually deals with (a 200% screen at 2880x1920 next to a 150% one at
 * 1920x1080, side by side in x as Windows arranges monitors).
 */
import { describe, expect, it, vi } from "vitest";
import type { MonitorArea, OverlaySlot, WidgetRecord } from "./lib";

const { invokeMock, listenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

// The overlay page's own window: `isOverlayMode()` answers true from the label
// alone, which is what keeps these tests away from `window.location`.
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ label: "desktop-overlay" }),
  currentMonitor: () => Promise.resolve(null),
  PhysicalPosition: class {
    constructor(public x: number, public y: number) {}
  },
  PhysicalSize: class {
    constructor(public w: number, public h: number) {}
  },
}));

/** The 200% screen: 2880x1920 physical is 1440x960 logical, at the origin. */
const SCREEN_A = {
  key: "A",
  name: "\\\\.\\DISPLAY1",
  physical: { x: 0, y: 0, w: 2880, h: 1920 },
  logical: { x: 0, y: 0, w: 1440, h: 960 },
  scale: 2,
  primary: true,
};

/** The 150% screen: 1920x1080 physical is 1280x720 logical, 2880px to the right. */
const SCREEN_B = {
  key: "B",
  name: "\\\\.\\DISPLAY2",
  physical: { x: 2880, y: 0, w: 1920, h: 1080 },
  logical: { x: 1440, y: 0, w: 1280, h: 720 },
  scale: 1.5,
  primary: false,
};

const SCREENS = [SCREEN_A, SCREEN_B];

/** The desktop as a whole, in logical px: the union of the two screens. */
const DESKTOP: MonitorArea = { x: 0, y: 0, w: 2720, h: 960 };

type Lib = typeof import("./lib");

/**
 * Re-import lib.ts against a fresh screen model. Its caches (screens, overlay
 * area, monitors, desktop) are module state filled from IPC, so each test starts
 * from a clean module.
 *
 * `cold` makes `floaty_screens` fail, which is the documented state of a page
 * that has asked about its own screen but has no screen list yet.
 */
async function boot(which: "A" | "B" = "A", opts: { cold?: boolean } = {}): Promise<Lib> {
  vi.resetModules();
  invokeMock.mockReset();
  listenMock.mockReset();
  listenMock.mockResolvedValue(() => undefined);
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "floaty_screens":
        return opts.cold ? Promise.reject(new Error("no screen list")) : Promise.resolve(SCREENS);
      case "floaty_overlay_area":
        return Promise.resolve({
          index: which === "A" ? 0 : 1,
          screen: which === "A" ? SCREEN_A : SCREEN_B,
          desktop: DESKTOP,
        });
      case "floaty_monitors":
        return Promise.resolve([SCREEN_A.logical, SCREEN_B.logical]);
      case "floaty_desktop_rect":
        return Promise.resolve(DESKTOP);
      default:
        return Promise.resolve(null);
    }
  });
  const lib = await import("./lib");
  await lib.screens();
  await lib.myArea();
  await lib.watchMonitors();
  return lib;
}

function slot(id: string, x: number, y: number, w: number, h: number): OverlaySlot {
  // preventOverlap reads the slot's geometry only; the element is never touched.
  return { id, kind: "note", x, y, w, h, element: null as unknown as HTMLElement };
}

function overlaps(
  cx: number,
  cy: number,
  w: number,
  h: number,
  o: { x: number; y: number; w: number; h: number },
): boolean {
  const ox = Math.min(cx + w, o.x + o.w) - Math.max(cx, o.x);
  const oy = Math.min(cy + h, o.y + o.h) - Math.max(cy, o.y);
  return ox > 12 && oy > 12;
}

describe("physicalToVirtual", () => {
  it("maps a point on the 200% screen through its scale", async () => {
    const lib = await boot();
    expect(lib.physicalToVirtual({ x: 1000, y: 800 })).toEqual({ x: 500, y: 400 });
  });

  it("maps a point on the 150% screen through its own scale and origin", async () => {
    const lib = await boot();
    // 300 physical px past the 200% screen's right edge is 200 logical px into
    // the 150% screen, whose own origin is 1440 record px out.
    expect(lib.physicalToVirtual({ x: 3180, y: 300 })).toEqual({ x: 1640, y: 200 });
  });

  it("puts the seam between the screens at 2880 physical", async () => {
    const lib = await boot();
    expect(lib.physicalToVirtual({ x: 2879, y: 10 })).toEqual({ x: 1439.5, y: 5 });
    expect(lib.physicalToVirtual({ x: 2880, y: 300 })).toEqual({ x: 1440, y: 200 });
  });

  it("returns null for a physical point on no screen", async () => {
    const lib = await boot();
    expect(lib.physicalToVirtual({ x: 5000, y: 100 })).toBeNull(); // right of everything
    expect(lib.physicalToVirtual({ x: 100, y: 2000 })).toBeNull(); // below the 200% screen
    expect(lib.physicalToVirtual({ x: 100, y: -1 })).toBeNull(); // above everything
  });

  it("falls back to this window's own screen when the list is cold", async () => {
    const lib = await boot("A", { cold: true });
    expect(lib.physicalToVirtual({ x: 1000, y: 800 })).toEqual({ x: 500, y: 400 });
    // the other screen is not this window's, and with no list there is nothing else to try
    expect(lib.physicalToVirtual({ x: 3180, y: 300 })).toBeNull();
  });
});

describe("physical -> virtual -> physical round trip", () => {
  /**
   * The inverse the mapping documents: `physical.x + (v.x - logical.x) * scale`.
   * lib.ts has no function for this direction — a record is only ever turned into
   * a physical point by the window that owns it, through its own scale factor —
   * so it is written here from the same screen table the app is fed.
   */
  function virtualToPhysical(
    v: { x: number; y: number },
    screen: (typeof SCREENS)[number],
  ): { x: number; y: number } {
    return {
      x: screen.physical.x + (v.x - screen.logical.x) * screen.scale,
      y: screen.physical.y + (v.y - screen.logical.y) * screen.scale,
    };
  }

  it("returns the original physical point on the 200% screen", async () => {
    const lib = await boot();
    const points = [
      { x: 0, y: 0 },
      { x: 1, y: 1 },
      { x: 1000, y: 800 },
      { x: 1440, y: 960 },
      { x: 2879, y: 1919 },
    ];
    for (const p of points) {
      const v = lib.physicalToVirtual(p);
      expect(v, `physical ${p.x},${p.y} should map`).not.toBeNull();
      const back = virtualToPhysical(v!, SCREEN_A);
      expect(back.x).toBeCloseTo(p.x, 6);
      expect(back.y).toBeCloseTo(p.y, 6);
    }
  });

  it("returns the original physical point on the 150% screen", async () => {
    const lib = await boot();
    const points = [
      { x: 2880, y: 0 },
      { x: 4000, y: 500 },
      { x: 4799, y: 1079 },
    ];
    for (const p of points) {
      const v = lib.physicalToVirtual(p);
      expect(v, `physical ${p.x},${p.y} should map`).not.toBeNull();
      const back = virtualToPhysical(v!, SCREEN_B);
      expect(back.x).toBeCloseTo(p.x, 6);
      expect(back.y).toBeCloseTo(p.y, 6);
    }
  });

  it("survives a point that lands on the seam, either side of it", async () => {
    const lib = await boot();
    const left = lib.physicalToVirtual({ x: 2879, y: 100 });
    const right = lib.physicalToVirtual({ x: 2880, y: 100 });
    expect(virtualToPhysical(left!, SCREEN_A).x).toBeCloseTo(2879, 6);
    expect(virtualToPhysical(right!, SCREEN_B).x).toBeCloseTo(2880, 6);
  });
});

describe("toWindow / toRecords", () => {
  it("are the identity when this window covers the screen at the origin", async () => {
    const lib = await boot("A");
    expect(lib.toWindow(500, 400)).toEqual({ x: 500, y: 400 });
    expect(lib.toRecords(500, 400)).toEqual({ x: 500, y: 400 });
  });

  it("offset by the second screen's own origin", async () => {
    const lib = await boot("B");
    // the 150% screen's window starts 1440 record px to the right of the arrangement's
    expect(lib.toWindow(1640, 200)).toEqual({ x: 200, y: 200 });
    expect(lib.toRecords(200, 200)).toEqual({ x: 1640, y: 200 });
  });

  it("round trip record -> window -> record on the second screen", async () => {
    const lib = await boot("B");
    for (const p of [
      { x: 1440, y: 0 },
      { x: 1640, y: 200 },
      { x: 2719, y: 719 },
    ]) {
      const w = lib.toWindow(p.x, p.y);
      expect(lib.toRecords(w.x, w.y)).toEqual(p);
    }
  });
});

describe("onMyScreen / isOnMyScreen", () => {
  it("claims only the screen this window covers", async () => {
    const lib = await boot("A");
    expect(lib.onMyScreen(0, 0)).toBe(true);
    expect(lib.onMyScreen(1439, 959)).toBe(true);
    expect(lib.onMyScreen(1440, 100)).toBe(false); // the other screen starts here
    expect(lib.onMyScreen(100, 960)).toBe(false); // below this screen
    expect(lib.onMyScreen(-1, 0)).toBe(false);
  });

  it("is the second screen's band when this window is on it", async () => {
    const lib = await boot("B");
    expect(lib.onMyScreen(1440, 0)).toBe(true);
    expect(lib.onMyScreen(2719, 719)).toBe(true);
    expect(lib.onMyScreen(2720, 0)).toBe(false);
    expect(lib.onMyScreen(100, 100)).toBe(false);
  });

  it("isOnMyScreen is the same answer", async () => {
    const lib = await boot("A");
    expect(lib.isOnMyScreen(100, 100)).toBe(lib.onMyScreen(100, 100));
    expect(lib.isOnMyScreen(2000, 100)).toBe(lib.onMyScreen(2000, 100));
  });
});

describe("monitorAt", () => {
  it("picks the screen whose horizontal band contains x", async () => {
    const lib = await boot("A");
    expect(lib.monitorAt(700)).toEqual(SCREEN_A.logical);
    expect(lib.monitorAt(1439)).toEqual(SCREEN_A.logical);
    expect(lib.monitorAt(1440)).toEqual(SCREEN_B.logical); // the seam belongs to the right screen
    expect(lib.monitorAt(2719)).toEqual(SCREEN_B.logical);
  });

  it("falls back to this window's own screen off the layout", async () => {
    const lib = await boot("B");
    expect(lib.monitorAt(5000)).toEqual(SCREEN_B.logical);
    expect(lib.monitorAt(-10)).toEqual(SCREEN_B.logical);
  });
});

describe("monitorAreaSync", () => {
  it("is the desktop rectangle the backend reported", async () => {
    const lib = await boot();
    expect(lib.monitorAreaSync()).toEqual(DESKTOP);
  });
});

describe("preventOverlap", () => {
  it("leaves a widget entirely inside the screen alone", async () => {
    const lib = await boot();
    lib.overlaySlots.set("other", slot("other", 1500, 300, 120, 120));
    expect(lib.preventOverlap({ id: "new", x: 100, y: 100, w: 120, h: 120, mon: DESKTOP })).toEqual({
      x: 100,
      y: 100,
    });
  });

  it("brings a widget whose whole body hangs off the right edge back inside the union", async () => {
    const lib = await boot();
    const r = lib.preventOverlap({ id: "new", x: 2900, y: 200, w: 120, h: 120, mon: DESKTOP });
    expect(r.x).toBe(DESKTOP.w - 120 - 16); // 2584
    expect(r.x + 120).toBeLessThanOrEqual(DESKTOP.w);
    expect(r.x).toBeGreaterThanOrEqual(16);
    expect(r.y).toBe(200);
  });

  it("trims a widget that only partly hangs off the right edge", async () => {
    const lib = await boot();
    expect(lib.preventOverlap({ id: "new", x: 2700, y: 200, w: 120, h: 120, mon: DESKTOP })).toEqual({
      x: 2584,
      y: 200,
    });
  });

  it("clamps a widget that hangs off the top-left", async () => {
    const lib = await boot();
    expect(lib.preventOverlap({ id: "new", x: -400, y: -400, w: 120, h: 120, mon: DESKTOP })).toEqual({
      x: 16,
      y: 16,
    });
  });

  it("confines to the screen it was given, not the union", async () => {
    const lib = await boot("B");
    const r = lib.preventOverlap({ id: "new", x: 5000, y: 100, w: 120, h: 120, mon: SCREEN_B.logical });
    expect(r.x).toBe(1440 + 1280 - 120 - 16); // 2584
    expect(r.x).toBeGreaterThanOrEqual(1440 + 16); // never left of the second screen
  });

  it("nudges a widget off a widget it would land on", async () => {
    const lib = await boot();
    const other = slot("other", 300, 100, 120, 120);
    lib.overlaySlots.set("other", other);
    const r = lib.preventOverlap({ id: "new", x: 400, y: 100, w: 120, h: 120, mon: DESKTOP });
    expect(r).toEqual({ x: 432, y: 100 });
    expect(overlaps(r.x, r.y, 120, 120, other)).toBe(false);
  });

  it("ignores the widget's own slot when looking for collisions", async () => {
    const lib = await boot();
    lib.overlaySlots.set("new", slot("new", 100, 100, 120, 120));
    expect(lib.preventOverlap({ id: "new", x: 100, y: 100, w: 120, h: 120, mon: DESKTOP })).toEqual({
      x: 100,
      y: 100,
    });
  });

  it("accepts the legacy positional form", async () => {
    const lib = await boot();
    expect(lib.preventOverlap("new", 2900, 200, 120, 120, { mon: DESKTOP })).toEqual({
      x: 2584,
      y: 200,
    });
  });
});

describe("findShiftCandidate", () => {
  it("picks the nearest non-colliding shift", async () => {
    const lib = await boot();
    const obstacle = slot("o", 300, 100, 150, 150);
    const ctx = {
      curX: 400,
      curY: 100,
      w: 100,
      h: 100,
      gap: 8,
      clampX: (v: number) => v,
      clampY: (v: number) => v,
    };
    const collides = (cx: number, cy: number) => overlaps(cx, cy, 100, 100, obstacle);
    // the obstacle ends at 450, so 462 is the shortest way clear of it
    expect(lib.findShiftCandidate([obstacle], ctx, collides)).toEqual({ x: 462, y: 100 });
  });

  it("takes the next-nearest shift when the nearest is blocked too", async () => {
    const lib = await boot();
    const obstacle = slot("o", 300, 100, 150, 150);
    const ctx = {
      curX: 400,
      curY: 100,
      w: 100,
      h: 100,
      gap: 8,
      clampX: (v: number) => v,
      clampY: (v: number) => v,
    };
    const collides = (cx: number, cy: number) =>
      overlaps(cx, cy, 100, 100, obstacle) || cx >= 450;
    // right is blocked; down (400, 262) is 162 away, up (400, -12) is 112
    expect(lib.findShiftCandidate([obstacle], ctx, collides)).toEqual({ x: 400, y: -12 });
  });

  it("returns null when nothing is free", async () => {
    const lib = await boot();
    const ctx = {
      curX: 0,
      curY: 0,
      w: 10,
      h: 10,
      gap: 8,
      clampX: (v: number) => v,
      clampY: (v: number) => v,
    };
    expect(lib.findShiftCandidate([slot("o", 0, 0, 10, 10)], ctx, () => true)).toBeNull();
  });
});

describe("findGridCandidate", () => {
  const ctx = {
    curX: 100,
    curY: 100,
    minX: 16,
    minY: 16,
    clampX: (v: number) => v,
    clampY: (v: number) => v,
  };

  it("returns the nearest cell when nothing is in the way", async () => {
    const lib = await boot();
    expect(lib.findGridCandidate(ctx, () => false)).toEqual({ x: 16, y: 16 });
  });

  it("skips the blocked cells and takes the first free one in ring order", async () => {
    const lib = await boot();
    const blocked = new Set(["16,16", "16,132", "16,248"]);
    const collides = (cx: number, cy: number) => blocked.has(`${cx},${cy}`);
    expect(lib.findGridCandidate(ctx, collides)).toEqual({ x: 116, y: 16 });
  });

  it("gives up (null) when every cell in reach collides", async () => {
    const lib = await boot();
    expect(lib.findGridCandidate({ ...ctx, curX: 0, curY: 0, minX: 0, minY: 0 }, () => true)).toBeNull();
  });
});

describe("clampNum", () => {
  it("passes a value inside the range through", async () => {
    const lib = await boot();
    expect(lib.clampNum(5, 0, 10)).toBe(5);
  });

  it("clamps to the ends", async () => {
    const lib = await boot();
    expect(lib.clampNum(-5, 0, 10)).toBe(0);
    expect(lib.clampNum(15, 0, 10)).toBe(10);
  });

  it("returns the minimum for a non-finite value", async () => {
    const lib = await boot();
    expect(lib.clampNum(NaN, 3, 10)).toBe(3);
    expect(lib.clampNum(Infinity, 3, 10)).toBe(3);
    expect(lib.clampNum(-Infinity, 3, 10)).toBe(3);
  });
});

describe("settingNum", () => {
  it("keeps a finite number and replaces anything else with the fallback", async () => {
    const lib = await boot();
    expect(lib.settingNum(5, 1)).toBe(5);
    expect(lib.settingNum(0, 1)).toBe(0);
    expect(lib.settingNum("5", 1)).toBe(1);
    expect(lib.settingNum(NaN, 7)).toBe(7);
    expect(lib.settingNum(undefined, 2)).toBe(2);
    expect(lib.settingNum(null, 2)).toBe(2);
  });
});

describe("displayName", () => {
  it("strips the launcher suffixes Explorer hides", async () => {
    const lib = await boot();
    expect(lib.displayName("Arc.lnk")).toBe("Arc");
    expect(lib.displayName("Zoom Workplace.url")).toBe("Zoom Workplace");
    expect(lib.displayName("setup.EXE")).toBe("setup");
  });

  it("keeps a suffix that says what the file is", async () => {
    const lib = await boot();
    expect(lib.displayName("notes.txt")).toBe("notes.txt");
    expect(lib.displayName("holiday.zip")).toBe("holiday.zip");
  });

  it("keeps the real name when stripping would leave nothing", async () => {
    const lib = await boot();
    expect(lib.displayName(".lnk")).toBe(".lnk");
  });
});

describe("describeForConfirm", () => {
  it("names the record and the path it points at", async () => {
    const lib = await boot();
    const rec: WidgetRecord = {
      id: "app-1",
      kind: "shortcut",
      x: 0,
      y: 0,
      data: { name: "Arc.lnk", target: "C:\\Users\\erict\\Desktop\\Arc.lnk" },
    };
    expect(lib.describeForConfirm(rec)).toBe("Arc\nC:\\Users\\erict\\Desktop\\Arc.lnk");
  });

  it("falls back to the id when the record has no name", async () => {
    const lib = await boot();
    const rec: WidgetRecord = { id: "app-7", kind: "note", x: 0, y: 0, data: {} };
    expect(lib.describeForConfirm(rec)).toBe("app-7");
  });
});

describe("iconIsMissing", () => {
  it("treats a missing or placeholder url as missing", async () => {
    const lib = await boot();
    expect(lib.iconIsMissing(undefined)).toBe(true);
    expect(lib.iconIsMissing("")).toBe(true);
    expect(lib.iconIsMissing("none")).toBe(true);
  });

  it("never judges a stored icon url, only the backend can", async () => {
    const lib = await boot();
    expect(lib.iconIsMissing("http://asset.localhost/C%3A%2FUsers%2Fx.png")).toBe(false);
  });

  it("reads a legacy data url out of its PNG header", async () => {
    const lib = await boot();
    // a real PNG header: signature + IHDR + 64x64
    expect(lib.iconIsMissing("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEAAAABA")).toBe(false);
    // too short to hold a header at all
    expect(lib.iconIsMissing("data:image/png;base64,AAAA")).toBe(true);
  });
});

describe("presenceIsQuiet", () => {
  it("starts false until the backend says the machine has gone quiet", async () => {
    const lib = await boot();
    expect(lib.presenceIsQuiet()).toBe(false);
  });
});
