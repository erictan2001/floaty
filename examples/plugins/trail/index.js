/**
 * Trail — draw a path on the desktop and the icons line up along it.
 *
 * The mode is the whole trick. A widget only receives the mouse where its own
 * rectangle is: floaty makes the desktop click-through everywhere else, so
 * "drawing on the desktop" means the panel first becomes the desktop — its slot is
 * resized to the monitor area — and then it can capture a stroke anywhere on
 * screen. The stroke is drawn on a canvas hung off `document.body` (rather than
 * inside the panel) so it is not clipped by the panel.
 *
 * Two ways to use it, because they are two different intentions. **single** treats
 * every stroke as the instruction: the icons line up along the path you just drew,
 * at once, every time you draw one, and the mode stays open so you can keep moving
 * them. **multiple** is for laying several paths out at once — draw as many as you
 * like, then press done (or enter) and the icons go out along all of them. Both
 * replace whatever was arranged before, and escape leaves the desktop as it was.
 *
 * The surface the strokes go on is see-through, because you are deciding where the
 * icons go and they are what you are deciding it against.
 *
 * Placing an icon does not fight its physics. An icon that has come to rest has
 * stopped its own loop, so moving it from here sticks; an icon still in the air
 * would keep falling, which is what `pinned: true` plus `floaty_refresh` fix — the
 * refresh makes it re-read its record, and it mounts standing where it was put
 * instead of dropping to the floor. `pinned` is floaty's own "it came to rest
 * here", which is also why the arrangement survives a restart.
 */

const PANEL = { w: 268, h: 146 };
/** Shorter than this is a slip of the hand, not a path. */
const MIN_STROKE = 140;
/** How long the icons take to walk to their places. */
const TRAVEL_MS = 700;
/** Keep placed icons this far inside the desktop. */
const EDGE = 10;
/** Points closer together than this are folded into one while drawing. */
const POINT_STEP = 5;
/** Breathing room between two arranged icons. */
const GAP = 6;
/**
 * The surface is see-through from the moment the mode opens: you are deciding
 * where the icons go, so the icons stay in view while you draw.
 */
const BACKDROP = "rgba(11,10,20,.55)";

export default {
  async mount(root, id, api) {
    const rec = await api.record.load(id);
    if (!rec) {
      api.log(`trail ${id}: record not found`);
      return;
    }

    root.innerHTML = "";
    const style = document.createElement("style");
    style.textContent = `
      .tr-panel{width:100%;height:100%;display:flex;flex-direction:column;border-radius:16px;
        background:linear-gradient(160deg,rgba(20,18,38,.84),rgba(28,24,52,.74));
        border:1px solid rgba(255,255,255,.13);overflow:hidden;
        font:12px/1.45 ui-sans-serif,system-ui,"Segoe UI",sans-serif;color:#ece9ff;
        text-shadow:0 1px 2px rgba(0,0,0,.5);user-select:none}
      .tr-bar{display:flex;align-items:center;gap:8px;padding:9px 12px 6px;
        font-size:11px;letter-spacing:.14em;text-transform:uppercase;opacity:.6;cursor:grab}
      .tr-bar .tr-title{flex:1}
      .tr-close{border:0;background:none;color:#ece9ff;opacity:.55;font:inherit;font-size:13px;
        line-height:1;padding:2px 4px;border-radius:6px;cursor:pointer}
      .tr-close:hover{opacity:1;background:rgba(255,255,255,.12)}
      .tr-body{padding:2px 12px 12px;display:flex;flex-direction:column;gap:7px}
      /* the hint takes the slack, so the buttons sit on the panel's bottom padding
         instead of running past it — the box is only 146px tall and a three-line
         hint pushed the row 4px out of the bottom */
      .tr-hint{margin:0;opacity:.72;flex:1}
      .tr-count{margin:0;opacity:.55;font-variant-numeric:tabular-nums}
      .tr-row{display:flex;gap:6px}
      .tr-btn{flex:1;padding:6px 8px;border-radius:9px;border:1px solid rgba(255,255,255,.16);
        background:rgba(255,255,255,.07);color:#ece9ff;font:inherit;cursor:pointer}
      .tr-btn:hover{background:rgba(255,255,255,.14)}
      .tr-btn[hidden]{display:none}
      .tr-btn.ghost{flex:0 0 auto;opacity:.75}
      .tr-note{position:fixed;left:50%;top:22px;transform:translateX(-50%);z-index:2147483001;
        padding:7px 14px;border-radius:999px;background:rgba(18,16,34,.86);color:#ece9ff;
        border:1px solid rgba(255,255,255,.15);font:12px/1.4 ui-sans-serif,system-ui,sans-serif;
        letter-spacing:.02em;pointer-events:none}
      .tr-done{position:fixed;left:50%;bottom:22px;transform:translateX(-50%);z-index:2147483001;
        display:flex;gap:8px;font:12px/1.4 ui-sans-serif,system-ui,sans-serif}
      .tr-done .tr-btn{flex:0 0 auto;min-width:78px;padding:7px 16px;border-radius:999px;
        background:rgba(18,16,34,.86);border:1px solid rgba(255,255,255,.15)}
      .tr-done .tr-btn:hover{background:rgba(34,30,60,.92)}
    `;
    root.append(style);

    const panel = document.createElement("div");
    panel.className = "tr-panel";
    const bar = document.createElement("div");
    bar.className = "tr-bar";
    const title = document.createElement("span");
    title.className = "tr-title";
    title.textContent = "trail";
    const close = document.createElement("button");
    close.type = "button";
    close.className = "tr-close";
    close.textContent = "×";
    close.title = "Remove this floatie";
    close.addEventListener("click", () => void api.removeSelf(rec));
    bar.append(title, close);
    const body = document.createElement("div");
    body.className = "tr-body";
    const hint = document.createElement("p");
    hint.className = "tr-hint";
    hint.textContent = "Single arranges as you draw; multiple waits for done. Drag the bar to move this panel.";
    const count = document.createElement("p");
    count.className = "tr-count";
    const row = document.createElement("div");
    row.className = "tr-row";

    const button = (label, cls, onClick) => {
      const b = document.createElement("button");
      b.className = `tr-btn${cls ? ` ${cls}` : ""}`;
      b.textContent = label;
      b.addEventListener("click", onClick);
      return b;
    };
    const singleBtn = button("single", "", () => void openMode("single"));
    const multiBtn = button("multiple", "", () => void openMode("multiple"));
    row.append(singleBtn, multiBtn);
    body.append(hint, count, row);
    panel.append(bar, body);
    root.append(panel);

    api.enableDrag(bar, rec);
    api.addPinMenu(panel, () => rec);

    /** The paths drawn so far, in desktop coordinates. */
    const trails = () => (Array.isArray(rec.data.trails) ? rec.data.trails : []);
    const issued = () => (Array.isArray(rec.data.ids) ? rec.data.ids.length : 0);
    const showCount = () => {
      const t = trails().length;
      const n = issued();
      const paths = t ? `${t} trail${t === 1 ? "" : "s"}` : "no trail";
      count.textContent = n ? `${paths} · ${n} icon${n === 1 ? "" : "s"}` : paths;
    };
    showCount();

    let drawing = false;

    // ---------- the path ----------

    /** Fold near-duplicate points out of a stroke: the maths below walks it. */
    const thin = (points) => {
      const out = [];
      for (const p of points) {
        const last = out[out.length - 1];
        if (!last || Math.hypot(p.x - last.x, p.y - last.y) >= POINT_STEP) out.push(p);
      }
      return out;
    };

    /** Length of the path up to each point: [0, d1, d1+d2, …]. */
    const lengths = (points) => {
      const cum = [0];
      for (let i = 1; i < points.length; i++) {
        cum.push(cum[i - 1] + Math.hypot(points[i].x - points[i - 1].x, points[i].y - points[i - 1].y));
      }
      return cum;
    };

    /** The point `at` pixels along the path. */
    const pointAt = (points, cum, at) => {
      const total = cum[cum.length - 1];
      if (at <= 0 || total <= 0) return { x: points[0].x, y: points[0].y };
      if (at >= total) return { x: points[points.length - 1].x, y: points[points.length - 1].y };
      for (let i = 1; i < cum.length; i++) {
        if (cum[i] >= at) {
          const span = cum[i] - cum[i - 1] || 1;
          const k = (at - cum[i - 1]) / span;
          return {
            x: points[i - 1].x + (points[i].x - points[i - 1].x) * k,
            y: points[i - 1].y + (points[i].y - points[i - 1].y) * k,
          };
        }
      }
      return { x: points[points.length - 1].x, y: points[points.length - 1].y };
    };

    /** How far along the path the closest point to `p` sits. */
    const nearest = (points, cum, p) => {
      let best = 0;
      let bestD = Infinity;
      for (let i = 0; i < points.length; i++) {
        const d = (points[i].x - p.x) ** 2 + (points[i].y - p.y) ** 2;
        if (d < bestD) {
          bestD = d;
          best = cum[i];
        }
      }
      return best;
    };

    /** Squared distance from `p` to the path, for picking the nearest trail. */
    const distanceTo = (points, p) => {
      let best = Infinity;
      for (const q of points) {
        const d = (q.x - p.x) ** 2 + (q.y - p.y) ** 2;
        if (d < best) best = d;
      }
      return best;
    };

    const overlaps = (a, b) =>
      a.x < b.x + b.w + GAP && b.x < a.x + a.w + GAP && a.y < b.y + b.h + GAP && b.y < a.y + a.h + GAP;

    // ---------- drawing mode ----------

    async function openMode(mode) {
      if (drawing) return;
      drawing = true;
      singleBtn.disabled = true;
      multiBtn.disabled = true;
      const area = await api.monitorArea();

      // The record is the truth about where the panel lives, and the panel may
      // have been dragged since this widget mounted — read it again, or "put it
      // back" puts it back to the wrong place.
      const fresh = await api.record.load(id);
      if (fresh) {
        rec.x = fresh.x;
        rec.y = fresh.y;
        rec.data = { ...fresh.data, ids: rec.data.ids };
      }
      const panelAt = { x: rec.x, y: rec.y };
      const panelSize = { w: rec.data.w || PANEL.w, h: rec.data.h || PANEL.h };
      // What the desktop already has. In multiple mode it is the context a new path
      // is drawn against — and it is about to be replaced. In single mode it is *not*
      // shown at all: a single-mode session replaces the arrangement, so leaving the
      // old paths painted makes the surface look like it has a second drawing on it,
      // which is exactly what it looked like.
      let context = mode === "multiple" ? trails() : [];
      // the paths of this session, in canvas coordinates; the record keeps them in
      // desktop coordinates, where the icons live. Kept apart from `context` until
      // they are committed, so escape is a real cancel.
      let session = [];
      let busy = false;
      let queued = false;

      // The panel becomes the desktop: this is what makes floaty report the mouse
      // to this widget at all, since the overlay is click-through outside the
      // widget's own rectangle.
      api.setPos(id, area.x, area.y);
      api.setSize(id, area.w, area.h);
      // The surface is its own thing. The panel — header, hint, count, buttons — goes
      // away for as long as a mode is open, so what you are looking at is a drawing
      // surface and not this widget stretched over the desktop. The slot itself stays
      // (it is what makes the desktop hand us the mouse); it is simply empty, and the
      // panel comes back exactly as it was when the mode ends.
      panel.style.display = "none";

      const canvas = document.createElement("canvas");
      const dpr = window.devicePixelRatio || 1;
      canvas.width = Math.round(area.w * dpr);
      canvas.height = Math.round(area.h * dpr);
      canvas.style.cssText = `position:fixed;left:0;top:0;width:${area.w}px;height:${area.h}px;` +
        `z-index:2147483000;cursor:crosshair;touch-action:none`;
      const ctx = canvas.getContext("2d");
      ctx.scale(dpr, dpr);

      const note = document.createElement("div");
      note.className = "tr-note";

      // multiple needs a way out that arranges; single arranges as it goes, so its
      // only button is the way out
      const doneWrap = document.createElement("div");
      doneWrap.className = "tr-done";
      const doneBtn = document.createElement("button");
      doneBtn.type = "button";
      doneBtn.className = "tr-btn";
      doneBtn.textContent = "done";
      doneBtn.hidden = mode === "single";
      const cancelBtn = document.createElement("button");
      cancelBtn.type = "button";
      cancelBtn.className = "tr-btn ghost";
      cancelBtn.textContent = "esc";
      doneWrap.append(doneBtn, cancelBtn);
      document.body.append(canvas, note, doneWrap);

      const points = [];
      let stroke = [];

      /** Draw a polyline, in the canvas's own coordinates. */
      const strokePath = (pts, colour, glow) => {
        if (pts.length < 2) {
          if (pts.length === 1) {
            ctx.beginPath();
            ctx.arc(pts[0].x, pts[0].y, 3, 0, Math.PI * 2);
            ctx.fillStyle = colour;
            ctx.fill();
          }
          return;
        }
        ctx.lineCap = "round";
        ctx.lineJoin = "round";
        ctx.lineWidth = glow ? 3 : 2;
        ctx.strokeStyle = colour;
        ctx.shadowColor = glow ? "rgba(150,130,255,.95)" : "transparent";
        ctx.shadowBlur = glow ? 18 : 0;
        ctx.beginPath();
        ctx.moveTo(pts[0].x, pts[0].y);
        // quadratic through the midpoints: the stroke the user drew, smoothed
        for (let i = 1; i < pts.length - 1; i++) {
          const mx = (pts[i].x + pts[i + 1].x) / 2;
          const my = (pts[i].y + pts[i + 1].y) / 2;
          ctx.quadraticCurveTo(pts[i].x, pts[i].y, mx, my);
        }
        const last = pts[pts.length - 1];
        ctx.lineTo(last.x, last.y);
        ctx.stroke();
        ctx.shadowBlur = 0;
      };

      const paint = () => {
        // clear first: a semi-transparent fill composites over what is already
        // there, so without this the surface would stay opaque and the icons it is
        // being drawn over would stay hidden
        ctx.clearRect(0, 0, area.w, area.h);
        ctx.fillStyle = BACKDROP;
        ctx.fillRect(0, 0, area.w, area.h);
        // in multiple mode, the paths already arranged are shown faintly: they are
        // what the new ones replace, and worth drawing against
        for (const t of context) {
          strokePath(
            t.map((p) => ({ x: p.x - area.x, y: p.y - area.y })),
            "rgba(236,233,255,.2)",
            false,
          );
        }
        // and the session's own path dims while a new stroke is being drawn, so there
        // is only ever one drawing on the surface that looks like *the* drawing
        for (const t of session) {
          strokePath(t, points.length ? "rgba(236,233,255,.2)" : "rgba(236,233,255,.5)", false);
        }
        strokePath(points, "rgba(236,233,255,.85)", true);
      };

      const idleNote = () => {
        if (mode === "single") {
          note.textContent = session.length
            ? "arranged · draw again to move them · enter or esc to finish"
            : "drag to draw the path the icons should stand on · enter or esc to finish";
          return;
        }
        note.textContent = session.length
          ? `${session.length} path${session.length === 1 ? "" : "s"} · done or enter to arrange · esc to cancel`
          : "drag to draw a path · done or enter to arrange · esc to cancel";
      };
      idleNote();

      const onDown = (e) => {
        if (e.button !== 0) return;
        points.length = 0;
        stroke = [];
        points.push({ x: e.clientX, y: e.clientY });
        stroke.push({ x: e.clientX, y: e.clientY });
        try {
          canvas.setPointerCapture(e.pointerId);
        } catch {
          /* a pointer that is already gone is not a reason to lose the stroke */
        }
        paint();
      };
      const onMove = (e) => {
        if (!points.length || e.buttons !== 1) return;
        points.push({ x: e.clientX, y: e.clientY });
        stroke.push({ x: e.clientX, y: e.clientY });
        paint();
      };
      // A finished stroke is kept, not acted on — except in single mode, where
      // drawing *is* the instruction: the icons follow the path you just drew,
      // every time you draw one, with nothing to confirm.
      const onUp = () => {
        if (!points.length) return;
        const thinned = thin(stroke);
        points.length = 0;
        if (thinned.length < 2 || lengths(thinned).at(-1) < MIN_STROKE) {
          note.textContent = "that was too short — draw a longer path";
          setTimeout(idleNote, 1400);
          paint();
          return;
        }
        if (mode === "single") {
          session = [thinned];
          paint();
          void arrange({ closeAfter: false });
          return;
        }
        session.push(thinned);
        idleNote();
        paint();
      };
      const onKey = (e) => {
        // enter always leaves the surface: in multiple mode it arranges first, in
        // single mode the arranging has already happened, stroke by stroke
        if (e.key === "Escape") void finish(true);
        if (e.key === "Enter") {
          if (mode === "single") void finish(true);
          else commit();
        }
      };

      /** "done" (or enter): arrange the icons along the paths drawn in this session. */
      const commit = () => {
        if (mode !== "multiple") return;
        if (session.length === 0) {
          note.textContent = "draw a path first";
          setTimeout(idleNote, 1400);
          return;
        }
        void arrange({ closeAfter: true });
      };
      doneBtn.addEventListener("click", commit);
      cancelBtn.addEventListener("click", () => void finish(false));

      canvas.addEventListener("pointerdown", onDown);
      canvas.addEventListener("pointermove", onMove);
      canvas.addEventListener("pointerup", onUp);
      window.addEventListener("keydown", onKey, true);
      paint();

      /** Leave drawing mode: drop the surface, put the panel back where it was. */
      const finish = async (keepTrail) => {
        canvas.remove();
        note.remove();
        doneWrap.remove();
        // and the panel is a panel again, not a drawing surface
        panel.style.display = "";
        window.removeEventListener("keydown", onKey, true);
        api.setSize(id, panelSize.w, panelSize.h);
        api.setPos(id, panelAt.x, panelAt.y);
        // and write that down: the panel is back where it was dragged to, so the
        // record has to say so too — otherwise it is the next mount (or the next
        // draw) that moves it, and the place it moves to is the stale one
        rec.x = panelAt.x;
        rec.y = panelAt.y;
        rec.data.w = panelSize.w;
        rec.data.h = panelSize.h;
        await api.record.save(rec);
        drawing = false;
        singleBtn.disabled = false;
        multiBtn.disabled = false;
        if (!keepTrail) api.log(`trail ${id}: drawing cancelled`);
      };

      /**
       * Arrange the icons along the paths drawn in this session.
       *
       * `closeAfter` is the difference between the two modes: multiple commits its
       * session and leaves, single arranges and stays put so the next stroke can
       * move the icons again.
       */
      const arrange = async (opts = {}) => {
        if (busy) {
          queued = true;
          return;
        }
        busy = true;
        doneBtn.disabled = true;
        const chosen = session.map((t) =>
          t.map((p) => ({ x: Math.round(p.x + area.x), y: Math.round(p.y + area.y) })),
        );
        // One failed arrangement must not wedge the mode for every drawing after it.
        // `busy` used to stay true when anything in there rejected, and from then on
        // every stroke was queued behind an arrangement that would never finish — which
        // is what "after the first draw it stops arranging" was.
        try {
          await place(chosen, opts);
        } catch (err) {
          const why = err && err.message ? err.message : String(err);
          api.log(`trail ${id}: arranging failed (${why}) — the next drawing will try again`);
        } finally {
          busy = false;
          if (drawing) doneBtn.disabled = false;
        }
        // a stroke drawn while the last one was still being arranged: do it now, rather
        // than leaving the icons where that one put them
        if (queued) {
          queued = false;
          if (drawing && !opts.closeAfter) void arrange({ closeAfter: false });
        }
      };

      /** Spread the desktop items along the trails and settle them there. */
      const place = async (paths, opts = {}) => {
        note.textContent = "arranging…";
        const all = paths;
        const cumByTrail = all.map((t) => lengths(t));

        const items = (await api.invoke("floaty_list")) || [];
        const manifest = (await api.invoke("floaty_plugins")) || [];
        const byKind = new Map(manifest.map((p) => [p.id, p]));
        // only the icons that stand for something on the desktop, and never this
        // panel: the trail arranges, it does not arrange itself
        const targets = items.filter((r) => {
          if (!r || r.id === id) return false;
          const p = byKind.get(r.kind);
          return !!(p && p.desktop_item && p.enabled !== false);
        });
        if (targets.length === 0) {
          note.textContent = "no icons to arrange — point floaty at a folder first";
          setTimeout(() => void finish(false), 1800);
          return;
        }

        const sizeOf = (r) => {
          const p = byKind.get(r.kind);
          const base = (p && p.default_size) || [92, 112];
          return { w: r.data.w || base[0], h: r.data.h || base[1] };
        };
        // nearest trail first, then along it: each icon claims the trail it already
        // sits closest to, in the order it already stands in, so nothing crosses the
        // desktop to get to its place
        const enriched = targets
          .map((r) => {
            const { w, h } = sizeOf(r);
            const centre = { x: r.x + w / 2, y: r.y + h / 2 };
            let best = { trail: 0, d: Infinity };
            all.forEach((t, i) => {
              const d = distanceTo(t, centre);
              if (d < best.d) best = { trail: i, d };
            });
            return { rec: r, w, h, trail: best.trail, d: best.d };
          })
          .sort((a, b) => a.trail - b.trail || a.d - b.d);

        // Share the icons between the trails by length, not by count: a short trail given
        // as many icons as a long one is cramped while the long one is sparse, and the
        // spacing on either side of a crossing stops matching. What is wanted is one gap
        // for the run on *every* trail, and that gap is (total length)/(icons) — so each
        // trail's share of the icons is its share of the length.
        const trailCount = all.length;
        const lens = cumByTrail.map((c) => c[c.length - 1]);
        const totalLen = lens.reduce((sum, v) => sum + v, 0) || 1;
        const exact = lens.map((l) => (enriched.length * l) / totalLen);
        const quota = exact.map((v) => Math.max(0, Math.floor(v)));
        let leftOver = enriched.length - quota.reduce((sum, v) => sum + v, 0);
        // hand out what the rounding left over, biggest fraction first
        const order = exact
          .map((v, i) => ({ i, frac: v - Math.floor(v) }))
          .sort((a, b) => b.frac - a.frac || lens[b.i] - lens[a.i]);
        for (const { i } of order) {
          if (leftOver <= 0) break;
          quota[i] += 1;
          leftOver -= 1;
        }
        // and no trail sits empty while there are icons to spare — an unpopulated trail
        // looks broken — so the shortest one takes a single icon off the longest
        if (enriched.length >= trailCount) {
          for (let i = 0; i < trailCount; i++) {
            if (quota[i] > 0) continue;
            let biggest = 0;
            for (let j = 0; j < trailCount; j++) if (quota[j] > quota[biggest]) biggest = j;
            if (quota[biggest] > 1) {
              quota[biggest] -= 1;
              quota[i] = 1;
            }
          }
        }

        // Who goes where: the icons closest to a trail fill it first, each taking its own
        // nearest trail while that trail still has room, otherwise the nearest one that
        // does. The counts come out of the length; the membership stays local, so nothing
        // crosses the desktop to reach its place.
        const claims = enriched
          .map((e) => {
            // inline: the shared helper is defined further down, and a claim is just a
            // centre to measure from
            const centre = { x: e.rec.x + e.w / 2, y: e.rec.y + e.h / 2 };
            const ds = all
              .map((t, i) => ({ i, d: distanceTo(t, centre) }))
              .sort((a, b) => a.d - b.d);
            return { entry: e, order: ds.map((x) => x.i), d: ds[0].d };
          })
          .sort((a, b) => a.d - b.d);
        const groups = all.map(() => []);
        const room = quota.slice();
        for (const c of claims) {
          const target = c.order.find((i) => room[i] > 0) ?? 0;
          if (room[target] > 0) room[target] -= 1;
          groups[target].push(c.entry);
        }

        const placed = [];
        const taken = [];
        let cursor = 0;
        /** Where an icon stands when its centre sits at `at` along the path. */
        const rectOn = (path, cum, at, entry) => {
          const p = pointAt(path, cum, at);
          return {
            at,
            w: entry.w,
            h: entry.h,
            x: Math.min(Math.max(p.x - entry.w / 2, area.x + EDGE), area.x + area.w - entry.w - EDGE),
            y: Math.min(Math.max(p.y - entry.h / 2, area.y + EDGE), area.y + area.h - entry.h - EDGE),
          };
        };
        const centreOf = (entry) => ({ x: entry.rec.x + entry.w / 2, y: entry.rec.y + entry.h / 2 });
        /**
         * The same place, nudged along its own path until no icon *from another trail* is
         * standing there. Within a single trail the run is evenly spaced and stays that
         * way: a trail too short for its icons reads better evenly spaced and slightly
         * touching than shoved about to separate pairs nobody can tell apart.
         */
        const clearOf = (path, cum, spot, entry, others) => {
          const clash = (r) => others.some((o) => o.trail !== entry.trail && overlaps(o, r));
          const cramped = (r) => others.some((o) => o.trail === entry.trail && overlaps(o, r));
          if (!clash(spot)) return spot;
          const stride = (entry.w + GAP) * 0.9;
          const end = cum[cum.length - 1];
          // Two looks: a place clear of the other trail *and* not crammed against a
          // trail-mate comes first, because a nudge that clears a crossing by landing on
          // your own neighbour has only moved the problem. If there is no such place, take
          // the least bad one — standing on the other trail is worse than a tight run.
          for (const picky of [true, false]) {
            for (let k = 1; k <= 12; k++) {
              for (const dir of [1, -1]) {
                const candidate = spot.at + dir * k * stride;
                if (candidate < 0 || candidate > end) continue;
                const r = rectOn(path, cum, candidate, entry);
                if (clash(r)) continue;
                if (picky && cramped(r)) continue;
                return r;
              }
            }
          }
          return spot;
        };

        all.forEach((path, ti) => {
          const group = groups[ti].slice();
          const cum = cumByTrail[ti];
          const total = cum[cum.length - 1];
          const inset = Math.min(26, total / 8);
          const span = Math.max(0, total - inset * 2);
          const n = group.length;
          // keep the order they already stand in along this path, so the run below
          // never has to move one past another
          group.sort((a, b) => nearest(path, cum, centreOf(a)) - nearest(path, cum, centreOf(b)));

          // Even spacing: one gap for every pair. Giving each pair exactly the room its
          // local direction demands is defensible pair by pair — and it is what this used
          // to do — but a path that steepens and then flattens comes out with icons bunched
          // where it steepened and sparse where it flattened, which reads as a bad
          // arrangement however sound each gap is on its own. One gap for the run is what
          // "distributed along the trail" looks like.
          const gap = n > 1 ? span / (n - 1) : 0;

          // The run can still slide by the two end margins without leaving the path, and
          // sliding it whole keeps every gap identical — so where another trail has already
          // been arranged, pick the phase that clashes least with it. This is the cheap way
          // to keep a crossing clear: phase the run rather than disturb it.
          const phase = (() => {
            if (n < 2 || inset < 1) return inset;
            let bestAt = inset;
            let bestHits = Infinity;
            for (let k = 0; k <= 8; k++) {
              const at = (2 * inset * k) / 8;
              let hits = 0;
              for (let i = 0; i < n; i++) {
                const r = rectOn(path, cum, at + gap * i, group[i]);
                for (const o of taken) {
                  if (overlaps(o, r)) hits += 1;
                }
              }
              if (hits < bestHits || (hits === bestHits && Math.abs(at - inset) < Math.abs(bestAt - inset))) {
                bestHits = hits;
                bestAt = at;
              }
            }
            return bestAt;
          })();

          const ats = [];
          for (let i = 0; i < n; i++) {
            ats.push(n > 1 ? phase + gap * i : total / 2);
          }

          group.forEach((entry, i) => {
            const spot = clearOf(path, cum, rectOn(path, cum, ats[i], entry), entry, taken);
            taken.push({ x: spot.x, y: spot.y, w: entry.w, h: entry.h, trail: ti });
            placed.push({
              id: entry.rec.id,
              w: entry.w,
              h: entry.h,
              from: nearest(path, cum, centreOf(entry)),
              to: spot.at,
              path,
              cum,
              trail: ti,
              x: spot.x,
              y: spot.y,
            });
          });
        });

        /**
         * The crossings. Within a trail the run is even by construction; where two trails
         * cross, an icon can land on one from the other, and that is the one thing even
         * spacing cannot sort out on its own. Move whichever of the pair has the shorter
         * way out along its own path — nothing else moves, so the even runs stay even.
         */
        const moveAlong = (it) => {
          const end = it.cum[it.cum.length - 1];
          // A fine scan of the trail, nearest first, rather than a few coarse strides:
          // at a crossing the two trails converge, so a 50px step along one of them
          // barely changes the distance to an icon on the other, and the coarse walk
          // kept landing on the same blocked spots and giving up.
          const step = 4;
          const clear = (at) => {
            const spot = rectOn(it.path, it.cum, at, it);
            return placed.every(
              (o) => o === it || !overlaps(o, { x: spot.x, y: spot.y, w: it.w, h: it.h }),
            );
          };
          for (let k = 0; k * step <= end; k++) {
            for (const at of k === 0 ? [it.to] : [it.to + k * step, it.to - k * step]) {
              if (at < 0 || at > end) continue;
              if (!clear(at)) continue;
              it.to = at;
              const spot = rectOn(it.path, it.cum, at, it);
              it.x = spot.x;
              it.y = spot.y;
              return true;
            }
          }
          return false;
        };
        let stuck = 0;
        for (let round = 0; round < 8; round++) {
          stuck = 0;
          for (let a = 0; a < placed.length; a++) {
            for (let b = a + 1; b < placed.length; b++) {
              // only crossings here: within a trail the even run stays as it is
              if (placed[a].trail === placed[b].trail) continue;
              if (!overlaps(placed[a], placed[b])) continue;
              // the later one moves by default: it is the one that landed on the other
              if (!moveAlong(placed[b]) && !moveAlong(placed[a])) stuck++;
            }
          }
          if (stuck === 0) break;
        }

        // How much of the desktop the trails actually had room for. A trail that cannot
        // hold its icons at even spacing keeps the even spacing and says so — the log is
        // the only place that is visible.
        let tight = 0;
        for (let a = 0; a < placed.length; a++) {
          for (let b = a + 1; b < placed.length; b++) {
            if (placed[a].trail === placed[b].trail && overlaps(placed[a], placed[b])) tight++;
          }
        }
        if (stuck > 0) api.log(`trail ${id}: ${stuck} crossing pair(s) could not be separated`);
        if (tight > 0) {
          api.log(`trail ${id}: ${tight} pair(s) touch at even spacing — the trail is short for ${placed.length} icon(s)`);
        }

        // walk each icon along its trail to its place: the trail says where it
        // stands, and this is the icon following it there
        await new Promise((done) => {
          const startedAt = performance.now();
          const tick = () => {
            const k = Math.min(1, (performance.now() - startedAt) / TRAVEL_MS);
            const eased = 1 - (1 - k) ** 3;
            for (const it of placed) {
              const p = pointAt(it.path, it.cum, it.from + (it.to - it.from) * eased);
              api.setPos(
                it.id,
                Math.round(Math.min(Math.max(p.x - it.w / 2, area.x + EDGE), area.x + area.w - it.w - EDGE)),
                Math.round(Math.min(Math.max(p.y - it.h / 2, area.y + EDGE), area.y + area.h - it.h - EDGE)),
              );
            }
            if (k < 1) requestAnimationFrame(tick);
            else done();
          };
          requestAnimationFrame(tick);
        });

        // The icons are standing where the trail said: in multiple mode the session
        // is over and the surface goes now, before the records are written. Writing
        // them first is what used to leave "arranging…" sitting on screen over an
        // arrangement that had already finished.
        if (opts.closeAfter) await finish(true);
        else {
          // nothing is carried forward here: in single mode the session *is* the
          // arrangement, and painting it again as context is what drew it twice
          idleNote();
        }

        // The backend's record wants integers — `floaty_save` rejects a record whose
        // x/y is a float ("invalid type: floating point … expected i32"), and the
        // positions a trail produces are interpolations along a path. One rejected
        // record used to abort the loop and lose the whole arrangement: nothing was
        // persisted, so the icons' records stayed where they were while the widgets
        // were drawn on the trail, and dragging one then fought its own record.
        const ids = [];
        const refused = [];
        for (const it of placed) {
          try {
            const target = await api.record.load(it.id);
            if (!target) continue;
            target.x = Math.round(it.x);
            target.y = Math.round(it.y);
            // it stands here now: `pinned` is what keeps it from falling again, and
            // `arranged` tells the overlay's layout pass to leave it alone — without it
            // the even spacing of a trail reads as a pile of collisions on the next
            // launch and the whole run is moved back onto the icon grid.
            if (!target.data) target.data = {};
            target.data.pinned = true;
            target.data.arranged = true;
            await api.record.save(target);
            ids.push(it.id);
          } catch (err) {
            const why = err && err.message ? err.message : String(err);
            refused.push(`${it.id} (${why})`);
          }
        }
        if (refused.length > 0) {
          api.log(
            `trail ${id}: ${refused.length} of ${placed.length} record(s) refused -> ${refused.slice(0, 3).join(", ")}`,
          );
        }
        // and make them re-read those records, so an icon that was mid-fall when
        // it was placed does not land on the floor instead. A build of floaty that
        // predates this command is not a reason to fail: the icons are already
        // saved where they belong, they will simply sort themselves out on the
        // next mount.
        try {
          await api.invoke("floaty_refresh", { ids });
        } catch {
          api.log(`trail ${id}: floaty_refresh unavailable — the places are saved, the icons will follow on restart`);
        }

        // Put them where the trail says once more, after the rebuild. A widget that
        // re-reads its record comes back at the record's place while its slot can
        // already hold a newer one — and a drag starts from the slot, so an icon whose
        // widget and slot disagree is dragged from the wrong coordinate. Saying it
        // again costs nothing and leaves the two in step.
        for (const it of placed) {
          api.setPos(it.id, Math.round(it.x), Math.round(it.y));
        }

        rec.data.trails = all;
        rec.data.ids = ids;
        await api.record.save(rec);
        showCount();
        api.log(
          `trail ${id}: arranged ${ids.length} icon(s) along ${all.length} trail(s), longest ${Math.round(
            Math.max(...cumByTrail.map((c) => c[c.length - 1])),
          )}px`,
        );
      };
    }
  },

  describe(rec) {
    const t = Array.isArray(rec.data.trails) ? rec.data.trails.length : 0;
    const n = Array.isArray(rec.data.ids) ? rec.data.ids.length : 0;
    if (!t && !n) return undefined;
    return `${n} icon${n === 1 ? "" : "s"} on ${t} trail${t === 1 ? "" : "s"}`;
  },
};
