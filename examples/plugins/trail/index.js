/**
 * Trail — draw a path on the desktop and the icons line up along it.
 *
 * The mode is the whole trick. A widget only receives the mouse where its own
 * rectangle is: floaty makes the desktop click-through everywhere else, so
 * "drawing on the desktop" means the panel first becomes the desktop — its slot is
 * resized to the monitor area — and then it can capture a stroke anywhere on
 * screen. The stroke is drawn on a canvas hung off `document.body` (rather than
 * inside the panel) so it is not clipped by the panel or hidden behind the icons
 * it is about to move.
 *
 * Placing an icon does not fight its physics. An icon that has come to rest has
 * stopped its own loop, so moving it from here sticks; an icon still in the air
 * would keep falling, which is what `pinned: true` plus `floaty_refresh` fix — the
 * refresh makes it re-read its record, and it mounts standing where it was put
 * instead of dropping to the floor. `pinned` is floaty's own "it came to rest
 * here", which is also why the arrangement survives a restart.
 *
 * The trail itself is not saved: it is a tool for arranging, not a decoration, and
 * it goes when the panel shrinks back. Right-click an icon to drop it again if you
 * want it to fall.
 */

const PANEL = { w: 268, h: 146 };
/** Shorter than this is a slip of the hand, not a path. */
const MIN_STROKE = 160;
/** How long the icons take to walk to their places. */
const TRAVEL_MS = 700;
/** Keep placed icons this far inside the desktop. */
const EDGE = 10;
/** Points closer together than this are folded into one while drawing. */
const POINT_STEP = 5;

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
      .tr-bar{padding:9px 12px 6px;font-size:11px;letter-spacing:.14em;text-transform:uppercase;
        opacity:.6;cursor:grab}
      .tr-body{padding:2px 12px 12px;display:flex;flex-direction:column;gap:7px}
      .tr-hint{margin:0;opacity:.72}
      .tr-count{margin:0;opacity:.55;font-variant-numeric:tabular-nums}
      .tr-row{display:flex;gap:6px}
      .tr-btn{flex:1;padding:6px 8px;border-radius:9px;border:1px solid rgba(255,255,255,.16);
        background:rgba(255,255,255,.07);color:#ece9ff;font:inherit;cursor:pointer}
      .tr-btn:hover{background:rgba(255,255,255,.14)}
      .tr-btn.ghost{flex:0 0 auto;opacity:.75}
      .tr-note{position:fixed;left:50%;top:22px;transform:translateX(-50%);z-index:2147483001;
        padding:7px 14px;border-radius:999px;background:rgba(18,16,34,.86);color:#ece9ff;
        border:1px solid rgba(255,255,255,.15);font:12px/1.4 ui-sans-serif,system-ui,sans-serif;
        letter-spacing:.02em;pointer-events:none}
    `;
    root.append(style);

    const panel = document.createElement("div");
    panel.className = "tr-panel";
    const bar = document.createElement("div");
    bar.className = "tr-bar";
    bar.textContent = "trail";
    const body = document.createElement("div");
    body.className = "tr-body";
    const hint = document.createElement("p");
    hint.className = "tr-hint";
    hint.textContent = "Draw a path and the app, file and folder icons line up along it.";
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
    const drawBtn = button("draw", "", () => void draw());
    const clearBtn = button("clear", "ghost", () => void clear());
    row.append(drawBtn, clearBtn);
    body.append(hint, count, row);
    panel.append(bar, body);
    root.append(panel);

    api.enableDrag(bar, rec);
    api.addPinMenu(panel, () => rec);

    const issued = () => (Array.isArray(rec.data.ids) ? rec.data.ids.length : 0);
    const showCount = () => {
      const n = issued();
      count.textContent = n ? `${n} icon${n === 1 ? "" : "s"} follow the trail` : "nothing arranged yet";
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

    // ---------- drawing mode ----------

    async function draw() {
      if (drawing) return;
      drawing = true;
      drawBtn.disabled = true;
      const area = await api.monitorArea();
      // The panel becomes the desktop: this is what makes floaty report the mouse
      // to this widget at all, since the overlay is click-through outside the
      // widget's own rectangle.
      const panelAt = { x: rec.x, y: rec.y };
      api.setPos(id, area.x, area.y);
      api.setSize(id, area.w, area.h);

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
      note.textContent = "drag to draw the path the icons should stand on · Esc to cancel";
      document.body.append(canvas, note);

      const points = [];
      let stroke = [];

      const paint = () => {
        ctx.clearRect(0, 0, area.w, area.h);
        if (points.length < 2) {
          if (points.length === 1) {
            ctx.beginPath();
            ctx.arc(points[0].x, points[0].y, 3, 0, Math.PI * 2);
            ctx.fillStyle = "rgba(236,233,255,.9)";
            ctx.fill();
          }
          return;
        }
        ctx.lineCap = "round";
        ctx.lineJoin = "round";
        ctx.lineWidth = 3;
        ctx.strokeStyle = "rgba(236,233,255,.85)";
        ctx.shadowColor = "rgba(150,130,255,.95)";
        ctx.shadowBlur = 18;
        ctx.beginPath();
        ctx.moveTo(points[0].x, points[0].y);
        // quadratic through the midpoints: the stroke the user drew, smoothed
        for (let i = 1; i < points.length - 1; i++) {
          const mx = (points[i].x + points[i + 1].x) / 2;
          const my = (points[i].y + points[i + 1].y) / 2;
          ctx.quadraticCurveTo(points[i].x, points[i].y, mx, my);
        }
        const last = points[points.length - 1];
        ctx.lineTo(last.x, last.y);
        ctx.stroke();
        ctx.shadowBlur = 0;
      };

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
      const onUp = () => {
        if (!points.length) return;
        const thinned = thin(stroke);
        if (thinned.length < 2 || lengths(thinned).at(-1) < MIN_STROKE) {
          note.textContent = "that was too short — draw a longer path";
          setTimeout(() => {
            note.textContent = "drag to draw the path the icons should stand on · Esc to cancel";
          }, 1400);
          return;
        }
        void place(thinned);
      };
      const onKey = (e) => {
        if (e.key === "Escape") void finish(false);
      };

      canvas.addEventListener("pointerdown", onDown);
      canvas.addEventListener("pointermove", onMove);
      canvas.addEventListener("pointerup", onUp);
      window.addEventListener("keydown", onKey, true);

      /** Leave drawing mode: drop the surface, put the panel back. */
      const finish = async (keepTrail) => {
        canvas.remove();
        note.remove();
        window.removeEventListener("keydown", onKey, true);
        api.setSize(id, PANEL.w, PANEL.h);
        api.setPos(id, panelAt.x, panelAt.y);
        drawing = false;
        drawBtn.disabled = false;
        if (!keepTrail) api.log(`trail ${id}: drawing cancelled`);
      };

      /** Spread the desktop items along the stroke and settle them there. */
      const place = async (path) => {
        note.textContent = "arranging…";
        const cum = lengths(path);
        const total = cum[cum.length - 1];
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
        // keep the order the icons are already in: each one claims the place
        // nearest where it stands now, so nothing crosses the desktop to get there
        const ordered = targets
          .map((r) => {
            const { w, h } = sizeOf(r);
            return { rec: r, w, h, from: nearest(path, cum, { x: r.x + w / 2, y: r.y + h / 2 }) };
          })
          .sort((a, b) => a.from - b.from);

        const inset = Math.min(18, total / 8);
        const step = ordered.length > 1 ? (total - inset * 2) / (ordered.length - 1) : 0;
        const placed = ordered.map((entry, i) => {
          const to = inset + step * i;
          const at = pointAt(path, cum, to);
          return {
            id: entry.rec.id,
            w: entry.w,
            h: entry.h,
            from: entry.from,
            to,
            x: at.x,
            y: at.y,
          };
        });

        // walk each icon along the path to its place: the trail says where it
        // stands, and this is the icon following it there
        const clamped = (p) => ({
          x: Math.min(Math.max(p.x - p.w / 2, area.x + EDGE), area.x + area.w - p.w - EDGE),
          y: Math.min(Math.max(p.y - p.h / 2, area.y + EDGE), area.y + area.h - p.h - EDGE),
        });
        await new Promise((done) => {
          const startedAt = performance.now();
          const tick = () => {
            const k = Math.min(1, (performance.now() - startedAt) / TRAVEL_MS);
            const eased = 1 - (1 - k) ** 3;
            for (const it of placed) {
              const p = pointAt(path, cum, it.from + (it.to - it.from) * eased);
              const at = clamped({ ...it, x: p.x, y: p.y });
              api.setPos(it.id, Math.round(at.x), Math.round(at.y));
            }
            if (k < 1) requestAnimationFrame(tick);
            else done();
          };
          requestAnimationFrame(tick);
        });

        const ids = [];
        for (const it of placed) {
          const target = await api.record.load(it.id);
          if (!target) continue;
          const at = clamped(it);
          target.x = Math.round(at.x);
          target.y = Math.round(at.y);
          // it stands here now: `pinned` is what keeps it from falling again, and
          // what makes the arrangement survive a restart
          target.data.pinned = true;
          await api.record.save(target);
          ids.push(it.id);
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

        rec.data.ids = ids;
        await api.record.save(rec);
        showCount();
        api.log(`trail ${id}: arranged ${ids.length} icon(s) along a ${Math.round(total)}px path`);
        await finish(true);
      };
    }

    async function clear() {
      rec.data.ids = [];
      await api.record.save(rec);
      showCount();
      api.log(`trail ${id}: cleared`);
    }
  },

  describe(rec) {
    const n = Array.isArray(rec.data.ids) ? rec.data.ids.length : 0;
    return n ? `${n} icon${n === 1 ? "" : "s"} on the trail` : undefined;
  },
};
