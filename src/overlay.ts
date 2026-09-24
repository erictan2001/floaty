import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import {
  currentSettings,
  isOverlayMode,
  isPinned,
  isTopLayer,
  beatNow,
  startHeartbeat,
  appWin,
  monitorArea,
  onMyScreen,
  physicalToVirtual,
  overlaySlots,
  preventOverlap,
  registerOverlaySlot,
  saveRecord,
  scheduleHitRectsUpdate,
  setWidgetPos,
  toWindow,
  unregisterOverlaySlot,
  watchSettings,
  type MonitorArea,
  type OverlaySlot,
  type WidgetRecord,
} from "./widgets/lib";
import {
  clearWidgetScope,
  loadPlugins,
  pluginApi,
  pluginFor,
  pluginKinds,
  widgetApi,
} from "./widgets/plugin";
import { crossCheckPlugins, isDesktopItem, layoutPriorityFor, pluginSize } from "./widgets/pluginManifest";

/**
 * Resolves initial desktop layout to prevent icons and folders from overlapping.
 * Existing valid in-bounds positions are preserved; any widgets at (0, 0), offscreen,
 * or colliding with earlier widgets are assigned clean, staggered desktop grid coordinates.
 */
export function resolveOverlayLayout(
  list: WidgetRecord[],
  mon: MonitorArea,
  top: boolean,
  opts: { tidy?: boolean } = {},
): Array<{ id: string; x: number; y: number }> {
  const screenW = mon.w > 0 ? mon.w : (window.innerWidth || 1920);
  const screenH = mon.h > 0 ? mon.h : (window.innerHeight || 1080);

  interface PlacedItem {
    id: string;
    x: number;
    y: number;
    w: number;
    h: number;
  }
  const placed: PlacedItem[] = [];
  const needsRelocation: WidgetRecord[] = [];
  // Tidy only: what is put back on the floor, so a stack of icons is not read as
  // a collision with itself. See the note on `tidy` below.
  const floorZone = new Set<string>();
  const moved: Array<{ id: string; x: number; y: number }> = [];

  const sorted = [...list].sort(
    (a, b) => layoutPriorityFor(a.kind) - layoutPriorityFor(b.kind),
  );

  for (const rec of sorted) {
    // Only this layer's widgets: the other layer's are laid out by its own page,
    // which is the one they are drawn in.
    if (isPinned(rec) !== top) continue;
    const { w, h } = pluginSize(rec);

    // "Tidy the desktop" is a deliberate instruction, so it overrides the two
    // things that normally protect a saved position — a hand-dragged one and an
    // arrangement a widget made (`arranged`). It applies to the icons only: a
    // note or a panel is hand-placed and stays where it is, which also makes it
    // something the grid must lay itself around.
    if (opts.tidy === true && isDesktopItem(rec.kind)) {
      if (isPinned(rec)) {
        needsRelocation.push(rec);
      } else {
        floorZone.add(rec.id);
        needsRelocation.push(rec);
      }
      continue;
    }

    // A record a widget placed on purpose is left exactly where it is. The trail widget
    // marks the icons it arranged (`data.arranged`), because the collision test below
    // allows only 40px of overlap — and the even spacing of a trail reads as a pile of
    // collisions on a curvy path, where icons a gap apart overlap vertically by more
    // than that. Without this the whole run was relocated to the icon grid on the next
    // launch, so an arrangement lasted exactly until the app was restarted.
    if (rec.data["arranged"] === true) {
      placed.push({ id: rec.id, x: rec.x, y: rec.y, w, h });
      continue;
    }

    // A saved position is the truth. The pass above only needs to give a place to a
    // record that does not have one — the backend writes a fresh widget at (0, 0) — so
    // anything past that is left exactly as it is, even a little off the screen or
    // overlapping a neighbour, because the user is the one who put it there. Testing
    // `inBounds` first was how a widget dragged 3px past the edge "lost" its position:
    // it failed the bounds test, was tidied onto the grid, and the tidy-up was saved
    // over what the user had chosen.
    const hasPos = rec.x > 10 || rec.y > 10;
    if (hasPos) {
      placed.push({ id: rec.id, x: rec.x, y: rec.y, w, h });
    } else {
      needsRelocation.push(rec);
    }
  }

  // Lay out needsRelocation across the desktop grid columns
  const MARGIN_LEFT = 24;
  const MARGIN_TOP = 24;
  const MARGIN_BOTTOM = 72;
  const CELL_W = 100;
  const CELL_H = 116;
  // The desktop has gravity, so a slot is only a place an icon will *stay* if
  // something holds it up. A pinned icon holds its own place; everything else
  // comes to rest on the floor, and a tidy that put one mid-screen would be a
  // tidy the next mount undid. So the grid runs two ways: pinned icons fill rows
  // from the top, and the rest stack up from the floor, one cell apart.
  const FLOOR_GAP = 6;

  const usableH = Math.max(200, screenH - MARGIN_TOP - MARGIN_BOTTOM);
  const rowsPerCol = Math.max(1, Math.floor(usableH / CELL_H));
  const floorRows = Math.max(
    1,
    Math.floor((screenH - MARGIN_TOP - FLOOR_GAP) / CELL_H),
  );

  let gridIndex = 0;
  let floorIndex = 0;
  for (const rec of needsRelocation) {
    const { w, h } = pluginSize(rec);
    const onFloor = opts.tidy === true && !isPinned(rec);
    // Clamp into the screen *before* testing for room. The clamp used to run
    // after, so any widget wider than a grid cell (a 240px plugin, the 392px
    // live2d) was pushed back onto the icon grid and ended up hidden behind the
    // tiles that were already there.
    let placedAt: { x: number; y: number } | undefined;
    for (let tries = 0; tries < 500 && !placedAt; tries++) {
      const index = onFloor ? floorIndex : gridIndex;
      const rows = onFloor ? floorRows : rowsPerCol;
      const col = Math.floor(index / rows);
      const row = index % rows;
      if (onFloor) {
        floorIndex++;
      } else {
        gridIndex++;
      }
      const gx = Math.max(MARGIN_LEFT, Math.min(MARGIN_LEFT + col * CELL_W, screenW - w - 16));
      // from the floor upwards for an icon that rests, from the top down for one
      // that is held: both are measured from the edge the icon answers to
      const gy = onFloor
        ? Math.max(MARGIN_TOP, screenH - FLOOR_GAP - h - row * CELL_H)
        : Math.max(MARGIN_TOP, Math.min(MARGIN_TOP + row * CELL_H, screenH - h - 16));

      const collides = placed.some((p) => {
        const ox = Math.min(gx + w, p.x + p.w) - Math.max(gx, p.x);
        const oy = Math.min(gy + h, p.y + p.h) - Math.max(gy, p.y);
        if (ox <= 12 || oy <= 12) return false;
        // A stack of resting icons is not a collision: the floor zone is what
        // the physics does with a pile, and tidy is only choosing the columns.
        return !floorZone.has(p.id);
      });
      if (!collides) placedAt = { x: gx, y: gy };
    }
    // a full desktop still gets the widget: last clamped spot rather than an
    // endless search
    placedAt ??= {
      x: Math.max(MARGIN_LEFT, Math.min(MARGIN_LEFT, screenW - w - 16)),
      y: Math.max(MARGIN_TOP, Math.min(MARGIN_TOP, screenH - h - 16)),
    };
    rec.x = placedAt.x;
    rec.y = placedAt.y;
    placed.push({ id: rec.id, x: rec.x, y: rec.y, w, h });
    moved.push({ id: rec.id, x: rec.x, y: rec.y });
    // What tidy placed has to survive the next launch, and the layout pass on
    // load keeps any record that says it was arranged on purpose — the same flag
    // the trail plugin sets for the icons it lines up.
    if (opts.tidy === true && isDesktopItem(rec.kind)) {
      rec.data["arranged"] = true;
    }
    void saveRecord(rec);
  }
  return moved;
}

/* ---------- the first-run guard (ADR 0004) ---------- */

/** What the backend says about the folder floaty is about to adopt. */
interface RootGuard {
  confirmed: boolean;
  root: string;
  exists: boolean;
  items: number;
  files: number;
  folders: number;
  apps: number;
  names: string[];
  more: number;
}

/** A pile of kebab-case css properties, so the card needs no stylesheet. */
function dress(el: HTMLElement, styles: Record<string, string>): void {
  for (const [prop, value] of Object.entries(styles)) el.style.setProperty(prop, value);
}

/** One sentence about what is in the folder — the thing the screen exists to say. */
function describeRoot(report: RootGuard): string {
  if (!report.root) return "No folder is set yet.";
  if (!report.exists)
    return "That folder is not there — moved, renamed, or on a drive that is not plugged in.";
  if (report.items === 0) return "That folder is empty.";
  const plural = (n: number, what: string): string => `${n} ${what}${n === 1 ? "" : "s"}`;
  const parts: string[] = [];
  if (report.files > 0) {
    const launchable = report.apps > 0 ? `, ${report.apps} of them launchable` : "";
    parts.push(`${plural(report.files, "file")}${launchable}`);
  }
  if (report.folders > 0) parts.push(plural(report.folders, "folder"));
  return `${plural(report.items, "item")}: ${parts.join(" and ")}.`;
}

/** The first few names, and how many were left out. */
function describeNames(report: RootGuard): string {
  if (report.names.length === 0) return "";
  const more = report.more > 0 ? `, and ${report.more} more` : "";
  return `${report.names.join(", ")}${more}`;
}

/**
 * The first-run guard: block until the user has been told what adopting the root
 * means, and has answered.
 *
 * Adopting a folder means floaty treats its contents as the desktop, and a merge moves
 * real files inside it. On the first launch there is no way to know that, so the screen
 * shows the folder it is about to adopt and what is in it and waits for an answer — and
 * nothing is drawn until there is one: the desktop is not merely covered, it is not
 * mounted at all. Every later root change warns and proceeds instead, because a step
 * the user has learned to click through is worse than no step at all.
 *
 * Resolves when the desktop may be drawn: at once when the question was already
 * answered, and after the answer when it was not. Both layers wait — the always-on-top
 * one has pinned floaties to draw, and "nothing is drawn until the user answers" has to
 * mean the whole desktop.
 */
async function firstRunGuard(root: HTMLElement): Promise<void> {
  let report: RootGuard;
  try {
    report = await invoke<RootGuard>("floaty_root_guard", { root: "" });
  } catch (e) {
    // A backend that cannot answer cannot show the folder either, and a screen that
    // blocks on a guess would lock a working desktop behind a question with nothing on
    // it. Say so in the log and let the desktop come up.
    void invoke("floaty_log", {
      msg: `[overlay] first-run guard could not ask: ${String(e)}`,
    }).catch(() => undefined);
    return;
  }
  if (report.confirmed) return;

  /** The folder the user picked *in this screen*, held in the page until the answer. */
  let pending = "";
  /** What the card is currently showing, so a settings change does not redraw it. */
  let shown = "";
  let answered = false;
  let stop: (() => void) | undefined;

  await new Promise<void>((resolve) => {
    const veil = document.createElement("div");
    veil.className = "first-run-veil";
    dress(veil, {
      position: "fixed",
      inset: "0",
      "z-index": "900",
      // A wash, not a wall: the desktop behind is still clickable (nothing of ours is
      // on it), and the veil itself takes no clicks.
      background:
        "radial-gradient(120% 120% at 50% 0%, rgba(20, 16, 44, 0.5), rgba(8, 6, 20, 0.68))",
      "pointer-events": "none",
    });

    const card = document.createElement("div");
    card.className = "first-run-guard";
    dress(card, {
      position: "fixed",
      left: "50%",
      top: "50%",
      transform: "translate(-50%, -50%)",
      "z-index": "901",
      width: "min(560px, calc(100vw - 64px))",
      "max-height": "calc(100vh - 64px)",
      "overflow-y": "auto",
      padding: "24px 26px 20px",
      "border-radius": "var(--panel-radius)",
      border: "1px solid var(--panel-border)",
      background: "var(--panel-bg)",
      "box-shadow": "0 18px 48px rgba(8, 6, 20, 0.45)",
      color: "var(--panel-ink)",
      "pointer-events": "auto",
      "font-size": "13px",
      "line-height": "1.5",
    });

    const eyebrow = document.createElement("div");
    eyebrow.textContent = "first run";
    dress(eyebrow, {
      "font-size": "10.5px",
      "letter-spacing": "0.14em",
      "text-transform": "uppercase",
      color: "var(--panel-ink-faint)",
      "margin-bottom": "8px",
    });

    const title = document.createElement("h1");
    title.textContent = "floaty is about to adopt this folder";
    dress(title, {
      margin: "0 0 14px",
      "font-size": "17px",
      "font-weight": "700",
      color: "var(--panel-ink)",
    });

    const path = document.createElement("div");
    dress(path, {
      padding: "9px 11px",
      "border-radius": "10px",
      background: "var(--panel-inset)",
      border: "1px solid var(--panel-hairline)",
      "font-family": "Consolas, 'Cascadia Mono', monospace",
      "font-size": "12px",
      "word-break": "break-all",
      "user-select": "text",
    });

    const summary = document.createElement("p");
    dress(summary, { margin: "12px 0 0" });

    const names = document.createElement("p");
    dress(names, { margin: "6px 0 0", color: "var(--panel-ink-soft)", "font-size": "12px" });

    const consequence = document.createElement("p");
    consequence.textContent =
      "Everything in it becomes your desktop — floaty watches the folder and shows what is in it. Dragging one item onto another merges them, and a merge moves real files, inside that folder, on disk.";
    dress(consequence, {
      margin: "14px 0 0",
      color: "var(--panel-ink-soft)",
      "font-size": "12px",
    });

    const error = document.createElement("p");
    dress(error, { margin: "10px 0 0", color: "var(--danger)", "font-size": "12px" });

    const row = document.createElement("div");
    dress(row, { display: "flex", gap: "10px", "margin-top": "18px", "flex-wrap": "wrap" });

    const confirm = document.createElement("button");
    confirm.type = "button";
    confirm.textContent = "adopt this folder";
    dress(confirm, {
      padding: "9px 16px",
      "border-radius": "999px",
      border: "1px solid transparent",
      background: "var(--accent)",
      color: "#fff",
      "font-size": "12.5px",
      "font-weight": "700",
      cursor: "pointer",
    });

    const choose = document.createElement("button");
    choose.type = "button";
    choose.textContent = "choose a different folder…";
    dress(choose, {
      padding: "9px 16px",
      "border-radius": "999px",
      border: "1px solid var(--panel-border)",
      background: "var(--panel-inset)",
      color: "var(--panel-ink)",
      "font-size": "12.5px",
      "font-weight": "600",
      cursor: "pointer",
    });

    row.append(confirm, choose);
    card.append(eyebrow, title, path, summary, names, consequence, error, row);
    // The desktop layer is the one that asks: with two layers per screen the
    // always-on-top one would otherwise stack a second identical card on the first.
    if (!isTopLayer()) root.append(veil, card);

    const render = (fresh: RootGuard): void => {
      const key = `${fresh.root}|${fresh.exists}|${fresh.items}|${fresh.folders}|${fresh.files}|${fresh.apps}|${fresh.names.join(",")}`;
      if (key === shown) return;
      shown = key;
      path.textContent = fresh.root || "no folder set";
      summary.textContent = describeRoot(fresh);
      names.textContent = describeNames(fresh);
      error.textContent = "";
    };

    const finish = (): void => {
      if (answered) return;
      answered = true;
      stop?.();
      veil.remove();
      card.remove();
      // the card's click region goes with it: leaving it behind would swallow the
      // clicks where the user is now working
      scheduleHitRectsUpdate();
      resolve();
    };

    /**
     * Ask again, and drop the guard the moment the answer is there — whether it was
     * given here or in the settings window, which is where the folder picker already
     * lives.
     */
    const recheck = async (): Promise<void> => {
      let fresh: RootGuard;
      try {
        fresh = await invoke<RootGuard>("floaty_root_guard", { root: pending });
      } catch {
        return;
      }
      if (fresh.confirmed) {
        finish();
        return;
      }
      render(fresh);
    };

    confirm.addEventListener("click", () => {
      confirm.disabled = true;
      void invoke<RootGuard>("floaty_confirm_root", { root: pending })
        .then(() => finish())
        .catch((e: unknown) => {
          confirm.disabled = false;
          error.textContent = `the answer did not stick: ${String(e)}`;
        });
    });

    choose.addEventListener("click", () => {
      void (async () => {
        try {
          const { open } = await import("@tauri-apps/plugin-dialog");
          const picked = await open({ directory: true, multiple: false });
          if (typeof picked !== "string" || !picked) return;
          // shown, not stored: picking a folder here is not adopting it — the
          // Confirm button is the only thing that does that
          pending = picked;
          await recheck();
        } catch (e: unknown) {
          error.textContent = `the folder picker failed: ${String(e)}`;
        }
      })();
    });

    render(report);
    // The card has to be clickable, and the overlay window is click-through
    // everywhere that is not a registered rect (see `syncHitRectsToRust`).
    scheduleHitRectsUpdate();
    // The folder can also be pointed at from the settings window, and the flag can be
    // set from another page: both arrive here as a settings change.
    listen("floaty-settings-changed", () => {
      void recheck();
    })
      .then((off) => {
        stop = off;
        // the guard may have been answered while this listener was being set up
        if (answered) off();
      })
      .catch(() => undefined);
    void invoke("floaty_log", {
      msg: `[overlay] first run: waiting for an answer about '${report.root || "(no folder set)"}'`,
    }).catch(() => undefined);
  });
}

export function mountOverlay(root: HTMLElement): void {
  root.innerHTML = "";
  // Both of these belong to the page rather than to the desktop, so they start before
  // the guard does: a page that never reports in looks like a stuck one (see the
  // heartbeat repairs), and the guard is itself a settings consumer.
  watchSettings();
  startHeartbeat(appWin.label);
  void firstRunGuard(root).then(() => mountDesktop(root));
}

function mountDesktop(root: HTMLElement): void {
  const top = isTopLayer();
  root.innerHTML = "";

  const canvas = document.createElement("div");
  canvas.id = "desktop-canvas";
  canvas.className = "overlay-canvas";
  root.append(canvas);

  const mountedSlots = new Set<string>();

  /* ---------- merge preview: the folder a drop would make ---------- */

  /**
   * Dragging one icon onto another is a real action — the backend folds the two
   * into a new folder in the pointed root, or puts the dragged item *inside* a
   * folder it lands on — and until now nothing said so until it had already
   * happened. Android's answer is the one people already know: the tile under
   * the finger becomes the folder it is about to be, its own icon shrinking into
   * one corner while the dragged one arrives in the other.
   *
   * The hit rule is copied from `floaty_dropped` on purpose (the dragged tile's
   * *centre* against every other desktop item's slot, each inflated by 12px), and
   * so is the tie-break, because a preview that disagrees with the drop would be
   * worse than none.
   */
  const MERGE_SLOP = 12;
  let mergeArmed: { target: string; dragged: string } | null = null;

  /** The tile a drop at this point would merge into, if any. */
  function mergeTargetAt(cx: number, cy: number, draggedId: string): OverlaySlot | null {
    // the backend walks its widget map, which serde keeps sorted by key, so two
    // pads covering the same centre resolve the same way here
    const candidates = [...overlaySlots.values()]
      .filter((s) => s.id !== draggedId && isDesktopItem(s.kind))
      .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
    for (const s of candidates) {
      const left = s.x - MERGE_SLOP;
      const top = s.y - MERGE_SLOP;
      const within =
        cx >= left &&
        cx < left + s.w + MERGE_SLOP * 2 &&
        cy >= top &&
        cy < top + s.h + MERGE_SLOP * 2;
      if (within) return s;
    }
    return null;
  }

  /**
   * Whatever the dragged tile is drawing — an app's icon, its letter, a folder's
   * glyph or first mini — cloned, so the preview shows the thing in hand rather
   * than a re-derived approximation of it.
   */
  function mergeGhostArt(dragged: OverlaySlot): Node | null {
    const pick = (sel: string): Element | null => dragged.element.querySelector(sel);
    const art =
      dragged.kind === "folder"
        ? (pick(".fminis img") ?? pick(".fminis .ffolder") ?? pick(".ffolder"))
        : (pick("img.tile-icon") ?? pick(".tile > span"));
    return art ? art.cloneNode(true) : null;
  }

  function disarmMerge(): void {
    if (!mergeArmed) return;
    const target = overlaySlots.get(mergeArmed.target);
    const dragged = overlaySlots.get(mergeArmed.dragged);
    const tile = target?.element.querySelector<HTMLElement>(".tile, .ftile");
    target?.element.classList.remove("merge-armed", "merge-two");
    target?.element.querySelector(".merge-ghost")?.remove();
    target?.element.querySelector(".merge-ring")?.remove();
    // the tile's own look comes back as it was: a letter tile paints a gradient
    // *inline*, which no stylesheet rule can override
    if (tile && tile.dataset.mergeBg !== undefined) {
      tile.style.background = tile.dataset.mergeBg;
      delete tile.dataset.mergeBg;
    }
    dragged?.element.querySelector(".launcher, .folder")?.classList.remove("merge-pulling");
    mergeArmed = null;
  }

  function armMerge(target: OverlaySlot, dragged: OverlaySlot): void {
    const tile = target.element.querySelector<HTMLElement>(".tile, .ftile");
    if (!tile) return;
    dragged.element.querySelector(".launcher, .folder")?.classList.add("merge-pulling");

    // the folder plate has to be the plate, not the icon's own gradient: appicon
    // paints letter tiles with an inline background, which beats the stylesheet
    if (tile.dataset.mergeBg === undefined) {
      tile.dataset.mergeBg = tile.style.background ?? "";
    }
    tile.style.background = "rgba(255, 255, 255, 0.3)";

    const ring = document.createElement("div");
    ring.className = "merge-ring";
    tile.appendChild(ring);

    const ghost = document.createElement("div");
    ghost.className = "merge-ghost";
    const art = mergeGhostArt(dragged);
    if (art) ghost.appendChild(art);

    // the incoming icon starts where the dragged tile is, so the eye follows it
    // into the folder instead of watching it appear
    const dx = dragged.x + dragged.w / 2 - (target.x + target.w / 2);
    const dy = dragged.y + dragged.h / 2 - (target.y + target.h / 2);
    ghost.style.setProperty("--from-x", `${Math.round(dx)}px`);
    ghost.style.setProperty("--from-y", `${Math.round(dy)}px`);

    if (target.kind === "folder") {
      // into the free cell of the folder's own 2x2 preview
      const used = tile.querySelectorAll(
        ".fminis img, .fminis .ffolder, .fminis .fmini-letter",
      ).length;
      ghost.classList.add(`slot-${used % 4}`);
    } else {
      // the tile itself becomes the two-icon folder preview, in the same
      // row-major order a folder's own mini grid uses (first icon top-left,
      // second top-right), so the plate matches the folder it is about to be
      target.element.classList.add("merge-two");
      ghost.classList.add("slot-1");
    }
    tile.appendChild(ghost);

    mergeArmed = { target: target.id, dragged: dragged.id };
    // arm on the next frame: inserted already-armed, the ghost would have no
    // starting position to animate from
    requestAnimationFrame(() => {
      if (mergeArmed?.target === target.id) target.element.classList.add("merge-armed");
    });
  }

  function refreshMerge(draggedId: string): void {
    const dragged = overlaySlots.get(draggedId);
    if (!dragged || !mountedSlots.has(draggedId)) {
      disarmMerge();
      return;
    }
    const target = mergeTargetAt(dragged.x + dragged.w / 2, dragged.y + dragged.h / 2, draggedId);
    if (!target) {
      disarmMerge();
      return;
    }
    if (mergeArmed && mergeArmed.target === target.id && mergeArmed.dragged === draggedId) return;
    disarmMerge();
    armMerge(target, dragged);
  }

  window.addEventListener("floaty-drag-move", (e) => {
    const detail = (e as CustomEvent<{ id?: string }>).detail;
    if (!detail?.id) return;
    refreshMerge(detail.id);
  });
  window.addEventListener("floaty-drag-end", () => disarmMerge());

  // A dragged floatie is moved by the window the drag started in, which after a boundary
  // crossing is no longer the window drawing it: this is how the drawing window follows
  // the pointer for the rest of the drag. Kept separate from `floaty-widget-updated`
  // below, which re-mounts the floatie — wrong per move, and slower.
  listen<{ id: string; x: number; y: number }>("floaty-drag-moved", (e) => {
    const moved = e.payload;
    if (!moved?.id || !mountedSlots.has(moved.id)) return;
    setWidgetPos(moved.id, moved.x, moved.y);
  }).catch(() => undefined);


  const mountWidget = (rec: WidgetRecord) => {
    if (mountedSlots.has(rec.id)) return;
    // Each layer draws only its own widgets: the desktop layer everything that
    // is not pinned, the always-on-top layer the ones that are. A widget is
    // mounted by exactly one of them (the backend tells both to reconcile when
    // the pin is toggled), which is what keeps a pinned widget from appearing in
    // the desktop layer as well.
    if (isPinned(rec) !== top) return;
    // One overlay per screen, and each draws only its own: a record whose place is on
    // another screen is that window's to mount, and this is what hands it over when a
    // drag crosses the boundary (the backend re-emits the record and both windows
    // re-decide).
    if (!onMyScreen(rec.x, rec.y)) return;
    const s = currentSettings();
    if (s.disabled && s.disabled.includes(rec.kind)) return;

    const plugin = pluginFor(rec.kind);
    if (!plugin) return;

    const { w, h } = pluginSize(rec);
    const slot = document.createElement("div");
    slot.className = "overlay-slot";
    slot.id = `slot-${rec.id}`;
    slot.dataset.id = rec.id;
    slot.dataset.kind = rec.kind;
    // record space → this window's css px (the identity on a single screen)
    const at = toWindow(rec.x, rec.y);
    slot.style.transform = `translate3d(${at.x}px, ${at.y}px, 0)`;
    slot.style.width = `${w}px`;
    slot.style.height = `${h}px`;

    canvas.append(slot);
    mountedSlots.add(rec.id);

    registerOverlaySlot({
      id: rec.id,
      kind: rec.kind,
      x: rec.x,
      y: rec.y,
      w,
      h,
      element: slot,
    });

    // mount may be async (installed plugins load their record first), so the
    // rejection has to be caught on the promise: a throw alone used to lose the
    // only clue a third-party plugin left behind.
    try {
      void Promise.resolve(plugin.mount(slot, rec.id, pluginApi(rec.kind))).catch((e: unknown) => {
        invoke("floaty_log", { msg: `[overlay] mount ${rec.id} failed: ${String(e)}` }).catch(
          () => undefined,
        );
      });
    } catch (e) {
      invoke("floaty_log", { msg: `[overlay] failed to mount ${rec.id}: ${String(e)}` }).catch(
        () => undefined,
      );
    }
  };

  const unmountWidget = (id: string) => {
    if (!mountedSlots.has(id)) return;
    // The widget is going away, so everything it started goes with it: a plugin's
    // timers and event listeners are keyed by the widget that owns them, and this
    // is the one place every removal path (removed, remounted, kind switched off)
    // comes through.
    clearWidgetScope(id);
    // a widget on its way out cannot be half of a preview
    if (mergeArmed && (mergeArmed.target === id || mergeArmed.dragged === id)) disarmMerge();
    mountedSlots.delete(id);
    unregisterOverlaySlot(id);
    const slot = document.getElementById(`slot-${id}`);
    if (slot) slot.remove();
  };

  // Initial load: the manifest first, so every slot is sized from the one place
  // the backend keeps sizes and no widget is laid out with a guess.
  void (async () => {
    try {
      const t0 = performance.now();
      await loadPlugins();
      const tPlugins = performance.now();
      crossCheckPlugins(pluginKinds());
      const list = await invoke<WidgetRecord[]>("floaty_list");
      const tList = performance.now();
      const mon = await monitorArea();
      const tMonitor = performance.now();
      const mine = list.filter((rec) => isPinned(rec) === top);
      invoke("floaty_log", {
        msg: `[overlay${top ? ":top" : ""}] init: plugins ${Math.round(tPlugins - t0)}ms, list(${list.length}) ${Math.round(
          tList - tPlugins,
        )}ms, mine(${mine.length}) monitor ${Math.round(tMonitor - tList)}ms, from page start ${Math.round(tMonitor)}ms`,
      }).catch(() => undefined);
      resolveOverlayLayout(list, mon, top);
      // Time each mount per kind: a reload's cost is dominated by whatever this
      // says, and the mount loop is synchronous, so the sum is the page's stall.
      const mountCost = new Map<string, number>();
      for (const rec of mine) {
        const t0 = performance.now();
        mountWidget(rec);
        mountCost.set(rec.kind, (mountCost.get(rec.kind) ?? 0) + (performance.now() - t0));
      }
      invoke("floaty_log", {
        msg:
          `[overlay${top ? ":top" : ""}] mount cost: ` +
          Array.from(mountCost)
            .sort((a, b) => b[1] - a[1])
            .map(([k, ms]) => `${k} ${Math.round(ms)}ms`)
            .join(", "),
      }).catch(() => undefined);
      scheduleHitRectsUpdate();
      // One line per launch: which kinds mounted, at which size, and where the
      // sizes came from. If the manifest did not arrive, every kind would be
      // missing here and the slots would be the 100x100 fallback.
      const summary = new Map<string, { n: number; size: string }>();
      for (const rec of mine) {
        const { w, h } = pluginSize(rec);
        const slot = summary.get(rec.kind) ?? { n: 0, size: `${w}x${h}` };
        slot.n += 1;
        summary.set(rec.kind, slot);
      }
      invoke("floaty_log", {
        msg: `[overlay${top ? ":top" : ""}] mounted ${mountedSlots.size}/${mine.length} floaties — ${Array.from(summary)
          .map(([kind, s]) => `${kind} ${s.n}x${s.size}`)
          .join(", ")}`,
      }).catch(() => undefined);
    } catch (e) {
      invoke("floaty_log", { msg: `[overlay] initial load failed: ${String(e)}` }).catch(
        () => undefined,
      );
    }
  })();

  // Listen for backend widget additions and removals
  listen<WidgetRecord>("floaty-widget-added", (e) => {
    if (e.payload) {
      const rec = e.payload;
      // Only a record the backend has never placed is tidied: a widget with a saved
      // position — one the user dragged, or one a widget arranged — keeps it, even off
      // the edge of the screen.
      const hasPos = rec.x > 10 || rec.y > 10;
      if (rec.data["pinned"] !== true && !hasPos) {
        const { w, h } = pluginSize(rec);
        const safe = preventOverlap(rec.id, rec.x, rec.y, w, h);
        if (safe.x !== rec.x || safe.y !== rec.y) {
          rec.x = safe.x;
          rec.y = safe.y;
          void saveRecord(rec);
        }
      }
      mountWidget(rec);
      scheduleHitRectsUpdate();
    }
  }).catch(() => undefined);

  // the backend renamed/changed a widget on disk: remount it so the label,
  // icon and hit rects match the new name
  // Files dropped from Explorer (or anywhere else) onto the desktop. The drop arrives as
  // a *physical* position with nothing else attached, so it goes through the same
  // per-screen mapping a drag uses and lands where it was dropped: on a folder floatie
  // that means inside the folder, anywhere else means onto the desktop.
  void (async () => {
    try {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      await getCurrentWebview().onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type !== "drop" || !payload.paths?.length) return;
        const at = physicalToVirtual({ x: payload.position.x, y: payload.position.y });
        if (!at) return;
        void invoke("floaty_drop_paths", {
          paths: payload.paths,
          x: Math.round(at.x),
          y: Math.round(at.y),
        }).catch(() => undefined);
      });
    } catch {
      /* the webview bridge is not there: nothing to drop onto */
    }
  })();

  listen<WidgetRecord>("floaty-widget-updated", (e) => {
    const rec = e.payload;
    if (!rec) return;
    unmountWidget(rec.id);
    mountWidget(rec);
    scheduleHitRectsUpdate();
  }).catch(() => undefined);

  listen<string>("floaty-widget-removed", (e) => {
    if (e.payload) {
      unmountWidget(e.payload);
      scheduleHitRectsUpdate();
    }
  }).catch(() => undefined);

  listen<Array<{ id: string; enabled: boolean }>>("floaty-plugins-changed", (e) => {
    const changes = e.payload || [];
    for (const p of changes) {
      if (!p.enabled) {
        // remove all widgets of this kind
        const toRemove: string[] = [];
        for (const slotId of mountedSlots) {
          const el = document.getElementById(`slot-${slotId}`);
          if (el && el.dataset.kind === p.id) {
            toRemove.push(slotId);
          }
        }
        toRemove.forEach(unmountWidget);
      } else {
        // re-mount widgets of this kind
        void invoke<WidgetRecord[]>("floaty_list").then((list) => {
          for (const rec of list) {
            if (rec.kind === p.id) {
              mountWidget(rec);
            }
          }
          scheduleHitRectsUpdate();
        });
      }
    }
  }).catch(() => undefined);

  // MutationObserver to detect DOM changes (popups, folder expansion) and sync hit rects
  const observer = new MutationObserver(() => {
    scheduleHitRectsUpdate();
  });
  observer.observe(canvas, { childList: true, subtree: true, attributes: true, attributeFilter: ["style", "class"] });

  // When window loses focus or user clicks outside, dismiss any menus
  window.addEventListener("blur", () => {
    const popups = document.querySelectorAll(".pin-menu, .model-menu");
    if (popups.length > 0) {
      popups.forEach((m) => m.remove());
      scheduleHitRectsUpdate();
    }
  });

  // Sleep/standby evidence: the backend does the recovery (it gets the display
  // power setting), but a hidden/visible transition is rare and is the only
  // record of what the page thought while the lid was shut.
  let hiddenSince = 0;
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      hiddenSince = Date.now();
      // Tell the backend now: it knows whether the display is actually on, and a
      // hidden page while the display is on means Chromium's occlusion verdict is
      // stale and this page's timers are about to be throttled.
      beatNow(appWin.label);
      invoke("floaty_log", { msg: `[overlay${top ? ":top" : ""}] hidden` }).catch(() => undefined);
      return;
    }
    const gap = hiddenSince ? Date.now() - hiddenSince : 0;
    hiddenSince = 0;
    // Coming back from hidden is exactly when the webview's surface can still be
    // holding its very first (white) frame in the regions this page paints
    // nothing into — the translucent tiles on top of it then show it as white
    // bands. Detaching the canvas for one frame makes Chromium repaint the whole
    // surface instead of only the damage it knows about.
    forceRepaint();
    beatNow(appWin.label);
    invoke("floaty_log", {
      msg: `[overlay${top ? ":top" : ""}] visible again after ${Math.round(gap / 1000)}s`,
    }).catch(() => undefined);
  });

  /**
   * Put this layer's icons back in order.
   *
   * Reuses the layout pass with `tidy`, which overrides saved positions and any
   * earlier arrangement for the icons (never for the panels), and then remounts
   * them so the desktop shows the new places instead of carrying on falling.
   *
   * The positions about to be overwritten are recorded as an undo step first:
   * this is the only place that has them, and a tidy is worth being able to take
   * back with Ctrl+Z.
   */
  const tidyDesktop = async (): Promise<number> => {
    const records = await invoke<WidgetRecord[]>("floaty_list");
    const mon = await monitorArea();
    const mine = records.filter(
      (rec) => isDesktopItem(rec.kind) && isPinned(rec) === top,
    );
    if (mine.length === 0) return 0;
    await invoke("floaty_undo_checkpoint", {
      label: "tidy",
      restore: mine.map((rec) => ({ id: rec.id, record: JSON.parse(JSON.stringify(rec)) })),
    }).catch(() => undefined);
    const moved = resolveOverlayLayout(records, mon, top, { tidy: true });
    if (moved.length > 0) {
      await invoke("floaty_refresh", { ids: moved.map((m) => m.id) }).catch(() => undefined);
    }
    return moved.length;
  };

  listen("floaty-tidy-requested", () => {
    void tidyDesktop()
      .then((count) => {
        void invoke("floaty_log", {
          msg: `[overlay${top ? ":top" : ""}] tidied ${count} floatie(s)`,
        });
        return emit("floaty-tidy-done", { count });
      })
      .catch((err) => {
        void invoke("floaty_log", {
          msg: `[overlay${top ? ":top" : ""}] tidy FAILED: ${String(err)}`,
        });
      });
  }).catch(() => undefined);

  // Ctrl+Z *while this window has focus* — which, being the desktop layer, it usually
  // does not: the window is no-activate by design, so it never takes focus from whatever
  // you are working in, and therefore never sees a keystroke. The shortcut that works from
  // anywhere is the global Ctrl+Alt+Z (see `UNDO_ACCELERATOR` in lib.rs). This handler is
  // kept because it costs nothing and covers the case where focus does land here.
  window.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === "z") {
      const active = document.activeElement;
      const typing =
        active instanceof HTMLElement &&
        (active.tagName === "INPUT" || active.tagName === "TEXTAREA" || active.isContentEditable);
      if (typing) return;
      e.preventDefault();
      void invoke<{ label: string; remaining: number }>("floaty_undo")
        .then((report) =>
          invoke("floaty_log", {
            msg: `[overlay] undid '${report.label}' (${report.remaining} left to undo)`,
          }),
        )
        .catch((err) =>
          invoke("floaty_log", { msg: `[overlay] undo: ${String(err)}` }),
        );
      return;
    }
    if (e.key === "Escape") {
      document.querySelectorAll(".pin-menu, .model-menu").forEach((m) => m.remove());
      document.querySelectorAll(".folder.open .fshut").forEach((b) => (b as HTMLElement).click());
      scheduleHitRectsUpdate();
    }
  });
}

/**
 * Make the whole surface paint again.
 *
 * A transparent WebView2 starts life painting a white frame of its own, and
 * Chromium only repaints the damage it is told about. Anything this page never
 * covers can therefore keep that white indefinitely — which reads as solid white
 * bands wherever a translucent tile sits over it. Hiding the canvas for one
 * frame invalidates the whole layer, so the next frame replaces it.
 */
export function forceRepaint(): void {
  const canvas = document.querySelector(".overlay-canvas");
  if (!(canvas instanceof HTMLElement)) return;
  const prev = canvas.style.visibility;
  canvas.style.visibility = "hidden";
  void canvas.offsetHeight;
  canvas.style.visibility = prev;
}
