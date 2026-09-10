import { PhysicalPosition } from "@tauri-apps/api/window";
import { appWin, currentSettings, loadRecord, logicalPos, monitorArea, removeSelf, saveRecord, watchSettings } from "./lib";

const WIN = 170; // must match Rust pet window size
const TICK_MS = 33;

export function mountPet(root: HTMLElement, id: string): void {
  watchSettings();
  const wrap = document.createElement("div");
  wrap.className = "pet-wrap";
  wrap.innerHTML = `
    <div class="pet" id="pet">
      <span class="zzz">z</span>
      <div class="eye left"><i></i></div>
      <div class="eye right"><i></i></div>
      <div class="mouth"></div>
    </div>
    <div class="pet-tag"><span class="pet-name"></span><button class="icon-btn pet-x" title="Remove">×</button></div>
  `;
  root.append(wrap);
  const pet = wrap.querySelector<HTMLElement>("#pet")!;
  // the × must never start a window drag
  wrap.querySelector(".pet-x")?.addEventListener("pointerdown", (e) => e.stopPropagation());

  let px = 200;
  let py = 200;
  let vx = 60;
  let vy = 40;
  let dragging = false;
  let hovering = false;
  let paused = false;
  let ready = false;
  let lastTouch = performance.now();
  let asleep = false;
  let suppressClickUntil = 0;
  let saveNow: (() => void) | undefined;

  // Geometry cache for the hot loop. Refreshed on a slow timer so the 30Hz
  // tick below stays fully synchronous (no awaits): the dragging flag is then
  // checked on the same JS turn that issues setPosition, which guarantees the
  // wander loop can never stomp an in-progress OS drag. Previously an
  // `await monitorArea()` sat between the check and setPosition, letting
  // stale ticks land mid-drag so the pet snapped back / refused to move —
  // and the ~120 IPC/sec flood starved other windows' invokes.
  let scale = 1;
  let mon = { x: 0, y: 0, w: 1280, h: 800 };
  const refreshGeom = async (): Promise<void> => {
    try {
      const s = await appWin.scaleFactor();
      if (s > 0) scale = s;
    } catch {
      /* keep last */
    }
    try {
      mon = await monitorArea();
    } catch {
      /* keep last */
    }
  };

  void (async () => {
    const rec = await loadRecord(id);
    if (rec) {
      const label = wrap.querySelector(".pet-name");
      if (label) label.textContent = typeof rec.data["name"] === "string" ? (rec.data["name"] as string) : "bloop";
      wrap.querySelector(".pet-x")?.addEventListener("click", (e) => {
        e.stopPropagation();
        void removeSelf(rec);
      });
      // keep place across restarts
      saveNow = () => {
        rec.x = Math.round(px);
        rec.y = Math.round(py);
        void saveRecord(rec);
      };
      window.setInterval(() => saveNow?.(), 4000);
      window.addEventListener("beforeunload", () => saveNow?.());
      document.addEventListener("visibilitychange", () => {
        if (document.hidden) saveNow?.();
      });
    }
    await refreshGeom();
    try {
      const p = await logicalPos();
      px = p.x;
      py = p.y;
    } catch {
      if (rec) {
        px = rec.x;
        py = rec.y;
      }
    }
    ready = true;
    window.setInterval(() => void refreshGeom(), 2000);
    void appWin.show().catch(() => undefined);
  })();

  const squash = () => {
    pet.classList.remove("squash");
    void pet.offsetWidth;
    pet.classList.add("squash");
  };

  pet.addEventListener("pointerenter", () => {
    hovering = true;
    lastTouch = performance.now();
    asleep = false;
    pet.classList.remove("sleeping");
    pet.classList.add("excited");
  });
  pet.addEventListener("pointerleave", () => {
    hovering = false;
    pet.classList.remove("excited");
  });
  pet.addEventListener("pointermove", (e) => {
    const r = pet.getBoundingClientRect();
    const dx = (e.clientX - (r.left + r.width / 2)) / r.width;
    const dy = (e.clientY - (r.top + r.height / 2)) / r.height;
    pet.style.setProperty("--gx", `${Math.max(-1, Math.min(1, dx)) * 6}px`);
    pet.style.setProperty("--gy", `${Math.max(-1, Math.min(1, dy)) * 5}px`);
  });
  pet.addEventListener("click", () => {
    if (performance.now() < suppressClickUntil) return; // was a drag, not a tap
    lastTouch = performance.now();
    squash();
  });
  // on `wrap`, not `pet`: pointer capture retargets post-drag clicks to the
  // capture element, and the × already stops propagation so it can't toggle.
  wrap.addEventListener("dblclick", () => {
    paused = !paused;
    lastTouch = performance.now();
    pet.classList.remove("spin");
    void pet.offsetWidth;
    pet.classList.add("spin");
  });
  // Manual drag: the window follows the cursor via pointermove and owns px/py
  // the whole time, so there is no position re-read that can go stale and no
  // OS-drag round-trip to race the wander loop. Whole window (pet + name tag)
  // is the handle; the × stops propagation above.
  wrap.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || dragging) return;
    e.stopPropagation();
    // NOTE: no preventDefault() here — canceling pointerdown suppresses the
    // compatibility mouse events, which would kill click/dblclick entirely.
    dragging = true;
    lastTouch = performance.now();
    asleep = false;
    pet.classList.remove("sleeping");
    squash();
    try {
      wrap.setPointerCapture(e.pointerId);
    } catch {
      /* capture unsupported — window-level listeners still cover the drag */
    }
    const startPX = px;
    const startPY = py;
    const startSX = e.screenX;
    const startSY = e.screenY;
    const clampDrag = () => {
      if (px < mon.x) px = mon.x;
      if (py < mon.y) py = mon.y;
      if (px > mon.x + mon.w - WIN) px = mon.x + mon.w - WIN;
      if (py > mon.y + mon.h - WIN) py = mon.y + mon.h - WIN;
    };
    const onMove = (ev: PointerEvent) => {
      px = startPX + (ev.screenX - startSX);
      py = startPY + (ev.screenY - startSY);
      clampDrag();
      lastTouch = performance.now();
      if (Math.hypot(ev.screenX - startSX, ev.screenY - startSY) > 4) {
        suppressClickUntil = performance.now() + 300;
      }
      void appWin
        .setPosition(new PhysicalPosition(Math.round(px * scale), Math.round(py * scale)))
        .catch(() => undefined);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      try {
        if (wrap.hasPointerCapture(e.pointerId)) wrap.releasePointerCapture(e.pointerId);
      } catch {
        /* ignore */
      }
      // px/py are already the drop point — nothing to re-read, just persist.
      saveNow?.();
      dragging = false;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  });
  pet.addEventListener("contextmenu", (e) => e.preventDefault());

  window.setInterval(() => {
    // hovered, paused, unloaded, or dragged => sit still
    if (!ready || dragging || hovering || paused || document.hidden) return;
    if (performance.now() - lastTouch > 60000 && !asleep) {
      asleep = true;
      pet.classList.add("sleeping");
    }
    const dt = TICK_MS / 1000;
    const speed = (asleep ? 0.15 : 1) * currentSettings().pet_speed;
    // gentle sine drift on top of straight wander
    const t = performance.now() / 1000;
    px += (vx * speed + Math.sin(t * 1.3) * 12) * dt;
    py += (vy * speed + Math.cos(t * 1.7) * 10) * dt;

    if (px < mon.x) { px = mon.x; vx = Math.abs(vx); }
    if (py < mon.y) { py = mon.y; vy = Math.abs(vy); }
    if (px > mon.x + mon.w - WIN) { px = mon.x + mon.w - WIN; vx = -Math.abs(vx); }
    if (py > mon.y + mon.h - WIN) { py = mon.y + mon.h - WIN; vy = -Math.abs(vy); }
    // fire-and-forget on purpose: the IPC send is issued synchronously on
    // this turn, so a pointerdown (same thread) can never slip between the
    // dragging check above and this call.
    void appWin
      .setPosition(new PhysicalPosition(Math.round(px * scale), Math.round(py * scale)))
      .catch(() => undefined);
  }, TICK_MS);
}
