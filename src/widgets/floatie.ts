/**
 * `floatie` — one floatie's place, its size, and the one gesture that moves it.
 *
 * A floatie's position used to live in three places at once: the record the store keeps,
 * the overlay slot that draws it, and a variable in whichever widget module happened to be
 * moving it. Nothing owned the fact, so every path that could disagree had a repair bolted
 * on. `saveRecord` copied the slot back over the record so a widget saving unrelated data
 * would not write a stale position; `trackPosition` polled the slot and saved it again;
 * appicon and folder each kept their own `x`/`y` and wrote the record from it on release.
 * The drag itself was written out four times, with two thresholds, three capture rules and
 * three release orderings, so every fix to the hard parts — placing from the pointer's own
 * position rather than from deltas, capturing on the body, the screen handover, the click
 * that follows a drag — had to be made in each of them.
 *
 * Here the record *is* the position. A slot is a view of it: the transform is written from
 * the record and never read back into it, so there is nothing to copy back and nothing to
 * poll. A module's own copy is a rendering cache, written in one place — `onPlace` — and
 * read by the module's own drawing. The gesture is one state machine, and its rules are
 * plain functions over plain data (`dragCrossed`, `placedAt`, `resizedTo`) so they can be
 * tested without a window. A widget that needs something of its own on release — settling
 * an icon, dropping it onto a folder — says so with a hook instead of writing a fifth drag.
 *
 * What this deliberately does not own: which screen draws the floatie (the backend owns
 * that and hands it between overlay windows through `floaty_drag_to`), the merge a drop may
 * cause (`floaty_dropped`), mounting (the overlay's job, and it creates the slot from the
 * record), and anything a module does with its own data.
 */
import { invoke } from "@tauri-apps/api/core";
import {
  appWin,
  dragGrab,
  dragPosition,
  isOverlayMode,
  logicalPos,
  notifyDragMove,
  notifyDragging,
  onMyScreen,
  overlaySlots,
  saveRecord,
  screens,
  setWidgetPos,
  setWidgetSize,
  type WidgetRecord,
} from "./lib";

/** A pointer event, in whichever space this page reads it. */
interface PointerAt {
  clientX: number;
  clientY: number;
  screenX?: number;
  screenY?: number;
}

/**
 * Where the pointer is, in the space this page's drags are measured in.
 *
 * An overlay window never moves, so its css px are the exact ones. A window-mode floatie
 * *does* move — it follows the pointer — so `clientX` stops advancing the moment the drag
 * starts and the deltas would be near zero; `screenX` is the one that keeps going. Both
 * appicon and folder learned this the hard way and each carried its own copy of the rule.
 */
export function pointerAt(ev: PointerAt): { x: number; y: number } {
  if (isOverlayMode()) return { x: ev.clientX, y: ev.clientY };
  return { x: ev.screenX ?? ev.clientX, y: ev.screenY ?? ev.clientY };
}

/** A press, from the pointer going down to it coming up. */
export interface DragState {
  /** Where the floatie was when the press began, in record space. */
  readonly startX: number;
  readonly startY: number;
  /** Where the pointer was when the press began, in this page's pointer space. */
  readonly startSX: number;
  readonly startSY: number;
  /**
   * The point of the floatie the pointer is holding, in record space. Placing the floatie
   * from the pointer's own position is what survives a change of scale part way through a
   * drag; `null` when there is no screen model to map through.
   */
  readonly grab: { dx: number; dy: number } | null;
  /** Past the threshold: this press is a drag, and not a click. */
  active: boolean;
  /** The last pointer position seen, for a release that carries none of its own. */
  lastX: number;
  lastY: number;
}

/**
 * Whether this move turns a press into a drag — the one threshold rule.
 *
 * Travel is a distance, not a box: a press that has moved 4px diagonally has moved 4px.
 * (The overlay drag tested the two axes separately, so a diagonal press could travel
 * further than the threshold without ever becoming a drag.)
 */
export function dragCrossed(
  state: DragState,
  at: { x: number; y: number },
  threshold: number,
): boolean {
  if (state.active) return true;
  return Math.hypot(at.x - state.startSX, at.y - state.startSY) > threshold;
}

/**
 * Where the floatie goes when the pointer is at `at` (record space) — the one placement
 * rule, so every drag places from the pointer's own position rather than from deltas.
 *
 * Deltas are exact while the pointer stays on one screen and wrong the moment it crosses
 * to another scale; the grab point is what survives that. With no point to map through —
 * a page that has not learned its screen yet, or a press that carried no grab — the
 * deltas are the fallback, and there they are exact.
 */
export function placedAt(
  state: DragState,
  at: { x: number; y: number } | null,
  dx: number,
  dy: number,
): { x: number; y: number } {
  if (at && state.grab) return { x: at.x - state.grab.dx, y: at.y - state.grab.dy };
  return { x: state.startX + dx, y: state.startY + dy };
}

/** A resize in flight: where the corner started, and how big the floatie was. */
export interface ResizeState {
  readonly startW: number;
  readonly startH: number;
  readonly startSX: number;
  readonly startSY: number;
}

/** How big a floatie becomes when its corner is dragged to this point — the one rule. */
export function resizedTo(
  state: ResizeState,
  curSX: number,
  curSY: number,
  minW: number,
  minH: number,
): { w: number; h: number } {
  return {
    w: Math.max(minW, state.startW + (curSX - state.startSX)),
    h: Math.max(minH, state.startH + (curSY - state.startSY)),
  };
}

/** What a module can say about its own floatie. */
export interface FloatieOptions {
  /** px of travel before a press counts as a drag (default 4). */
  threshold?: number;
  /** Called when a press is accepted, before the gesture begins (a tile's squash). */
  onPress?: () => void;
  /**
   * Everything of the module's own that a move must know about: its rendering cache, its
   * physics. Called after the record and the slot have been written, so `rec.x`/`rec.y` and
   * the arguments agree. This is the *only* place a module's copy of the position is
   * written.
   */
  onPlace?: (x: number, y: number) => void;
  /**
   * The module's own release work: settling an icon, dropping it onto a folder. Called for
   * every release, with `moved` false for a press that never became a drag. Return `true`
   * when the floatie is gone (a merge swallowed it) — there is then nothing left to persist.
   * The gesture is ended either way.
   */
  onRelease?: (ev: PointerEvent | undefined, moved: boolean) => boolean | Promise<boolean>;
  /**
   * A module's own confinement: a tile that clamps itself to its monitor says so here, so
   * the clamp happens once, in the one place a position is decided, instead of in each of
   * the module's move paths.
   */
  constrain?: (x: number, y: number) => { x: number; y: number };
  /** Controls inside the draggable surface that keep their own behaviour. */
  controls?: string;
  /**
   * How a press moves a *window-mode* floatie. `"os"` hands the drag to the window manager
   * (`startDragging`), which is what a widget with a title bar wants: the page gets no
   * moves to follow, and the record catches up in `watch`. `"manual"` moves the window from
   * the pointer, which is what a tile that clamps itself to its monitor wants. In overlay
   * mode the two are the same code: the floatie is a slot in a full-screen window, and the
   * window itself never moves.
   */
  windowDrag?: "os" | "manual";
}

/** The controls that keep their own behaviour inside a draggable surface. */
export const DRAG_CONTROLS =
  "button, input, textarea, select, .resize-handle, .pin-menu, .model-menu";

/** One floatie: its record, and the press that is moving it, if any. */
export class Floatie {
  private live: WidgetRecord;
  private drag: DragState | null = null;
  private captured: number | null = null;
  private swallowing: ((e: Event) => void) | null = null;
  private onPlace: ((x: number, y: number) => void) | undefined;
  private constrain: ((x: number, y: number) => { x: number; y: number }) | undefined;
  private watching = false;
  /** How to stop `watch` again: its timer and the two listeners it installed. */
  private stopWatch: (() => void) | null = null;
  /** True while a plugin has stretched this slot over the desktop as scaffolding. */
  private transient = false;
  /** px of travel before a press counts as a drag. */
  private threshold = 4;
  /** The window's scale factor, for the modes where a position is physical px. */
  private scale = 1;

  constructor(rec: WidgetRecord) {
    this.live = rec;
  }

  /**
   * The record this floatie is. A page that re-reads its record (`loadRecord` after a
   * change elsewhere) hands over a fresh copy, and the newest copy is the one the session
   * writes — a session holding a stale object would write a position nobody reads.
   */
  get rec(): WidgetRecord {
    return this.live;
  }

  /** Hand over the live record, which a reload replaces. */
  adopt(rec: WidgetRecord): void {
    this.live = rec;
  }

  /** Whether a press is moving this floatie right now. */
  get dragging(): boolean {
    return this.drag !== null;
  }

  /** The slot this window draws it in, if this window draws it at all. */
  private get slot() {
    return overlaySlots.get(this.rec.id);
  }

  /**
   * Where the floatie is. The record answers in both modes: in a window it is kept in step
   * with the window by `place` and `watch`, and in an overlay it is what the slot was drawn
   * from.
   */
  async position(): Promise<{ x: number; y: number }> {
    if (isOverlayMode()) return { x: this.rec.x, y: this.rec.y };
    return logicalPos();
  }

  /**
   * Put the floatie here: the caller is moving it. One fact, two views — the record (which
   * is persisted, and which the next mount draws from) and, in an overlay, the slot's
   * transform.
   *
   * `transient` marks a slot a plugin has stretched over the desktop as scaffolding rather
   * than as the widget's place: the position is not the user's choice, so it is drawn and
   * not recorded, and nothing watching may persist it.
   */
  place(x: number, y: number, opts?: { transient?: boolean; scale?: number }): void {
    this.transient = opts?.transient === true;
    // Only what is drawn travels down: `transient` has been read above, and passing it on
    // would put a flag back in `draw`'s options that nothing there looks at.
    const drawn = { scale: opts?.scale };
    if (this.transient) {
      // A stretched slot is scaffolding, not the widget's place: a plugin has covered the
      // desktop with something that catches the mouse (a drawing surface, a page of save
      // slots), and the place it covers is not where the widget lives. Drawn, not recorded
      // — and `watch` must not adopt it either, which is the hazard the old
      // `reservingSlots` set existed to prevent.
      this.draw(x, y, drawn);
      return;
    }
    this.rec.x = Math.round(x);
    this.rec.y = Math.round(y);
    this.draw(this.rec.x, this.rec.y, drawn);
  }

  /**
   * Draw the floatie at a place a module worked out itself, without touching the record.
   *
   * This is the physics: an icon falling writes its position every frame, and where it
   * *is* between two frames is not a fact worth persisting — the record gets the place it
   * comes to rest at. It is also the one path that must not round, because a body moving
   * less than a pixel a frame would stall if every frame's position were snapped.
   *
   * The options are what is drawn *with*, and nothing more: whether the place is scaffolding
   * rather than the widget's own is `place`'s question, and it answers it before it gets
   * here — the two flags once shared this signature, and carrying a `transient` that this
   * function never read is how the wrong one would eventually be consulted.
   */
  draw(x: number, y: number, opts?: { scale?: number }): void {
    // A slot is only moved while the floatie is on this window's screen. Past the edge the
    // other window draws it, and moving this one's transform to a place this window does
    // not cover would draw it in the band between the screens until the handover lands.
    if (!isOverlayMode() || onMyScreen(x, y)) {
      setWidgetPos(this.rec.id, x, y, opts?.scale ?? this.scale);
    }
    this.onPlace?.(x, y);
  }

  /**
   * How big it is: size lives in the record's data, and in the slot or the window.
   *
   * `transient` means the same thing here as it does in `place`: the size is
   * scaffolding — a panel stretched over the desktop to catch the mouse — and not
   * the widget's own size, so it is drawn and not recorded. Without it, stretching
   * the slot overwrote the size in the record, and the widget that put it back then
   * restored the size it read there: the monitor's. A plugin that saves its own
   * record afterwards made that permanent, so the widget came back full-desktop.
   * A restore is an ordinary `size()` — the widget really is that big again.
   */
  size(w: number, h: number, scale?: number, opts?: { transient?: boolean }): void {
    if (opts?.transient !== true) {
      this.rec.data["w"] = Math.round(w);
      this.rec.data["h"] = Math.round(h);
    }
    setWidgetSize(this.rec.id, w, h, scale ?? this.scale);
  }

  /**
   * Persist it. The record already says where the floatie is, so this is a save, not a
   * repair.
   */
  async commit(): Promise<void> {
    await saveRecord(this.rec);
  }

  /**
   * Keep a window-mode floatie's record in step with its window.
   *
   * A window moved by its own title bar — or by the window manager, or by `startDragging`,
   * which blocks this page until the move is over — is the one move the page cannot see
   * happen, so it is the one thing that still has to be looked at. In an overlay there is
   * nothing to watch: every move went through `place`, and the backend was told each one.
   */
  watch(): void {
    if (isOverlayMode() || this.watching) return;
    this.watching = true;
    const snap = async () => {
      if (this.dragging || this.transient) return;
      try {
        const p = await this.position();
        const x = Math.round(p.x);
        const y = Math.round(p.y);
        if (x !== this.rec.x || y !== this.rec.y) {
          // The window is already there: this is the record catching up with it, not a
          // move, so nothing is told to go anywhere.
          this.rec.x = x;
          this.rec.y = y;
          this.onPlace?.(x, y);
          await this.commit();
        }
      } catch {
        /* window gone */
      }
    };
    const timer = window.setInterval(() => void snap(), 2000);
    const onUnload = () => void snap();
    const onHide = () => {
      if (document.hidden) void snap();
    };
    window.addEventListener("beforeunload", onUnload);
    document.addEventListener("visibilitychange", onHide);
    this.stopWatch = () => {
      window.clearInterval(timer);
      window.removeEventListener("beforeunload", onUnload);
      document.removeEventListener("visibilitychange", onHide);
      this.watching = false;
      this.stopWatch = null;
    };
  }

  /**
   * Make an element drag this floatie around.
   *
   * A press only becomes a drag after `threshold` px of travel, so a widget that also
   * reacts to plain clicks (a visualizer's modes, a clock's date picker) keeps working:
   * a press that really moved swallows the click that follows it.
   */
  attachDrag(el: HTMLElement, opts: FloatieOptions = {}): void {
    if (opts.threshold !== undefined) this.threshold = opts.threshold;
    if (opts.onPlace) this.onPlace = opts.onPlace;
    if (opts.constrain) this.constrain = opts.constrain;
    const controls = opts.controls ?? DRAG_CONTROLS;
    const isControl = (target: EventTarget | null): boolean => {
      const t = target as HTMLElement | null;
      return !!t?.closest?.(controls);
    };
    void this.readScale();

    el.addEventListener(
      "pointerdown",
      (e) => {
        if (e.button !== 0) return;
        if (isControl(e.target)) return;
        if (this.drag) return; // a press already in flight owns the gesture
        this.stopSwallowing();
        if (!isOverlayMode() && opts.windowDrag !== "manual") {
          // The window manager owns this move: the page gets no moves to follow, and
          // `watch` is what writes the record afterwards.
          void appWin.startDragging().catch(() => undefined);
          return;
        }
        if (isOverlayMode() && !this.slot) return;
        e.stopPropagation();
        opts.onPress?.();
        // Announced before the first move: this is the snapshot of where the floatie was,
        // and after one move it would be the place the drag had already taken it to.
        void invoke("floaty_gesture_begin", { label: "move", ids: [this.rec.id] }).catch(
          () => undefined,
        );
        // Warm the screen list: the first move maps the pointer through it, and a cold
        // cache would fall back to the delta maths this exists to replace.
        void screens();
        void this.readScale();
        const start = pointerAt(e);
        const state: DragState = {
          startX: this.rec.x,
          startY: this.rec.y,
          startSX: start.x,
          startSY: start.y,
          grab: dragGrab(e, this.rec.x, this.rec.y),
          active: false,
          lastX: start.x,
          lastY: start.y,
        };
        this.drag = state;

        const onMove = (ev: PointerEvent) => {
          const at = pointerAt(ev);
          state.lastX = at.x;
          state.lastY = at.y;
          if (!state.active) {
            if (!dragCrossed(state, at, this.threshold)) return;
            state.active = true;
            // This is the moment a drag starts, and so the moment to capture the pointer:
            // on the *body*, not on the widget, because the widget's slot leaves the DOM
            // when the drag crosses onto another screen (that window owns it now), and
            // capture on a removed element stops delivering moves — the drag would freeze
            // at the boundary. Doing it here rather than at pointerdown is what keeps a tap
            // clickable: while the capture is held, `click` fires at the capturing element,
            // so the widget under the pointer never sees it.
            try {
              this.captured = ev.pointerId;
              (el.ownerDocument?.body ?? el).setPointerCapture(ev.pointerId);
            } catch {
              /* a browser without pointer capture: the drag still works inside the window */
            }
            this.swallowClick();
            notifyDragging(true);
          }
          this.moveTo(at, ev, state);
        };
        const onUp = (up?: PointerEvent) => {
          window.removeEventListener("pointermove", onMove);
          window.removeEventListener("pointerup", onUp);
          window.removeEventListener("pointercancel", onUp);
          // One last placement, so a drop just past the edge still lands on the screen the
          // pointer is on rather than on the last position the moves saw. The coordinates
          // must come from the release, or from the last move — never from the press, which
          // is what placed every floatie back where the drag began.
          //
          // It has to happen *before* the gesture ends. The backend re-homes a floatie the
          // release left on no screen, and a placement arriving after the commit writes the
          // off-screen position straight back on top of the repair: the log says "brought
          // back" and the floatie is still gone. Placing first also puts this last position
          // inside the gesture, so the one undo step holds it.
          if (state.active) {
            const at = pointerAt(up ?? { clientX: state.lastX, clientY: state.lastY });
            this.moveTo(at, up, state);
          }
          // One step for the gesture, if it changed anything at all.
          void invoke("floaty_gesture_end").catch(() => undefined);
          this.releaseCapture();
          const moved = state.active;
          if (!moved) this.drag = null;
          void (async () => {
            try {
              const gone = opts.onRelease ? await opts.onRelease(up, moved) : false;
              if (moved && !gone) await this.commit();
            } catch {
              /* a module's own release work must not leave the drag open */
            } finally {
              // The drag is over whatever the release did. Leaving this set would keep the
              // overlay's click-through region cleared until the next drag.
              this.drag = null;
              if (moved) {
                notifyDragging(false);
                // the click lands in the same gesture; drop the guard right after
                window.setTimeout(() => this.stopSwallowing(), 350);
              }
            }
          })();
        };
        window.addEventListener("pointermove", onMove);
        window.addEventListener("pointerup", onUp);
        window.addEventListener("pointercancel", onUp);
      },
      // capture, so a widget's own pointerdown handler (which stops propagation) cannot
      // swallow the drag
      true,
    );
  }

  /**
   * One move: tell the backend, place the floatie, tell the overlay where it is.
   *
   * The backend is told on every move because it owns which screen the floatie is on and
   * hands it between windows as that changes, in both directions. The overlay is told so a
   * drop here can show what it would do — it decides what that means for each kind.
   */
  private moveTo(
    at: { x: number; y: number },
    ev: PointerAt | undefined,
    state: DragState,
  ): void {
    const dx = at.x - state.startSX;
    const dy = at.y - state.startSY;
    const want = placedAt(state, ev ? dragPosition(ev) : null, dx, dy);
    const to = this.constrain ? this.constrain(want.x, want.y) : want;
    const x = Math.round(to.x);
    const y = Math.round(to.y);
    void invoke("floaty_drag_to", { id: this.rec.id, x, y }).catch(() => undefined);
    this.place(x, y);
    notifyDragMove(this.rec.id, x, y);
  }

  /** The bottom-right corner grip: a resize is a gesture like a drag. */
  resizeHandle(el: HTMLElement, minW: number, minH: number): void {
    el.style.position = "relative";
    const grip = document.createElement("div");
    grip.className = "resize-handle";
    grip.title = "Resize";
    el.append(grip);
    grip.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.stopPropagation();
      e.preventDefault();
      notifyDragging(true);
      // a resize is a gesture like a drag: snapshot the record before the first move
      void invoke("floaty_gesture_begin", { label: "resize", ids: [this.rec.id] }).catch(
        () => undefined,
      );
      void (async () => {
        const overlay = isOverlayMode();
        await this.readScale();
        const scale = this.scale;
        let startW = el.offsetWidth;
        let startH = el.offsetHeight;
        if (!overlay) {
          try {
            const sz = await appWin.innerSize();
            startW = sz.width / scale;
            startH = sz.height / scale;
          } catch {
            return;
          }
        }
        const start = pointerAt(e);
        const state: ResizeState = {
          startW,
          startH,
          startSX: start.x,
          startSY: start.y,
        };
        const onMove = (ev: PointerEvent) => {
          const at = pointerAt(ev);
          const { w, h } = resizedTo(state, at.x, at.y, minW, minH);
          setWidgetSize(this.rec.id, w, h, scale);
        };
        const onUp = () => {
          window.removeEventListener("pointermove", onMove);
          window.removeEventListener("pointerup", onUp);
          window.removeEventListener("pointercancel", onUp);
          void invoke("floaty_gesture_end").catch(() => undefined);
          notifyDragging(false);
          this.size(el.offsetWidth, el.offsetHeight, scale);
          void this.commit();
        };
        window.addEventListener("pointermove", onMove);
        window.addEventListener("pointerup", onUp);
        window.addEventListener("pointercancel", onUp);
      })();
    });
  }

  /** Stop watching this floatie and forget it (the overlay unmounting its slot). */
  dispose(): void {
    this.stopWatch?.();
    this.stopSwallowing();
    this.releaseCapture();
    this.drag = null;
    sessions.delete(this.rec.id);
  }

  private async readScale(): Promise<void> {
    try {
      const s = await appWin.scaleFactor();
      if (s > 0) this.scale = s;
    } catch {
      /* keep the last known scale */
    }
  }

  private swallowClick(): void {
    if (this.swallowing) return;
    // Installed at the window in the capture phase, because a listener on the widget itself
    // would run *after* the widget's own handler.
    const swallow = (e: Event): void => {
      e.stopPropagation();
      e.preventDefault();
    };
    this.swallowing = swallow;
    window.addEventListener("click", swallow, true);
  }

  private stopSwallowing(): void {
    if (!this.swallowing) return;
    window.removeEventListener("click", this.swallowing, true);
    this.swallowing = null;
  }

  private releaseCapture(): void {
    const id = this.captured;
    this.captured = null;
    if (id === null) return;
    try {
      (document.body ?? document.documentElement).releasePointerCapture?.(id);
    } catch {
      /* never captured */
    }
  }
}

const sessions = new Map<string, Floatie>();

/** The session for a record — one per floatie, made on demand. */
export function floatie(rec: WidgetRecord): Floatie {
  let session = sessions.get(rec.id);
  if (!session) {
    session = new Floatie(rec);
    sessions.set(rec.id, session);
  } else {
    // the caller's copy is the live one now: a page that re-read its record hands the
    // session the fresh object, and a session left holding the old one would write a
    // position nothing reads
    session.adopt(rec);
  }
  return session;
}
