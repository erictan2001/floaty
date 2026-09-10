import { invoke } from "@tauri-apps/api/core";
import { PhysicalPosition } from "@tauri-apps/api/window";
import {
  appWin,
  currentSettings,
  loadRecord,
  logicalPos,
  monitorArea,
  removeSelf,
  saveRecord,
  setLogicalPos,
  watchSettings,
  type MonitorArea,
  type WidgetRecord,
} from "./lib";

const WIN_W = 92;
const WIN_H = 112;
const REST_WALL = 0.6;

interface LayoutItem {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

const GRADIENTS = [
  ["#8b7bff", "#5b4bd6"],
  ["#4fc3f7", "#2470c7"],
  ["#5eead4", "#0d9488"],
  ["#fda4af", "#e11d63"],
  ["#fdba74", "#ea580c"],
  ["#fde047", "#ca8a04"],
];

function gradientFor(name: string): [string, string] {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return GRADIENTS[h % GRADIENTS.length] as [string, string];
}

export function mountLauncher(root: HTMLElement, id: string): void {
  watchSettings();
  const wrap = document.createElement("div");
  wrap.className = "launcher idle-hidden";
  wrap.innerHTML = `
    <button class="launcher-x" title="Remove">\u00d7</button>
    <div class="tile"><span></span></div>
    <div class="launcher-name"></div>
  `;
  root.append(wrap);
  const tile = wrap.querySelector<HTMLElement>(".tile")!;
  const letter = wrap.querySelector<HTMLElement>(".tile span")!;
  const nameEl = wrap.querySelector<HTMLElement>(".launcher-name")!;

  let rec: WidgetRecord | undefined;
  let x = 200;
  let y = 40;
  let vx = 0;
  let vy = 0;
  let settled = false;
  let dragging = false;
  let ready = false;
  let mon: MonitorArea = { x: 0, y: 0, w: 1280, h: 800 };
  let scale = 1;
  let others: LayoutItem[] = [];
  let last = performance.now();
  let clickTimer: number | undefined;
  let suppressClickUntil = 0;

  const squash = () => {
    tile.classList.remove("squash");
    void tile.offsetWidth;
    tile.classList.add("squash");
  };

  const saveSoon = () => {
    if (!rec) return;
    rec.x = Math.round(x);
    rec.y = Math.round(y);
    rec.data["pinned"] = settled;
    void saveRecord(rec).catch(() => undefined);
  };

  void (async () => {
    rec = await loadRecord(id);
    if (!rec) {
      wrap.remove();
      return;
    }
    const nm = typeof rec.data["name"] === "string" && rec.data["name"]
      ? (rec.data["name"] as string)
      : "app";
    const ch = (nm.trim()[0] ?? "?").toUpperCase();
    letter.textContent = ch;
    nameEl.textContent = nm;
    nameEl.title = nm;
    const [g1, g2] = gradientFor(nm);
    // bubble toggle: colored gradient tile on, bare floating icon off.
    // Must clear the inline background too — inline style beats the stylesheet.
    const setBubble = (on: boolean) => {
      tile.classList.toggle("real", !on);
      tile.style.background = on ? `linear-gradient(145deg, ${g1}, ${g2})` : "transparent";
      letter.style.display = on ? "" : "none";
    };
    setBubble(true);
    // real app icon when available (cached in the record, else resolved once
    // by the backend and persisted there); letter tile stays as the fallback
    const applyIcon = (url: string) => {
      setBubble(false);
      let img = tile.querySelector<HTMLImageElement>("img.tile-icon");
      if (!img) {
        img = document.createElement("img");
        img.className = "tile-icon";
        img.draggable = false;
        img.alt = "";
        tile.prepend(img);
      }
      img.src = url;
    };
    const cachedIcon = typeof rec.data["icon"] === "string" ? (rec.data["icon"] as string) : "";
    if (cachedIcon) {
      applyIcon(cachedIcon);
    } else {
      invoke<string>("floaty_icon", { id: rec.id }).then(applyIcon).catch(() => undefined);
    }
    wrap.classList.remove("idle-hidden");

    try {
      const p = await logicalPos();
      x = p.x;
      y = p.y;
    } catch {
      x = rec.x;
      y = rec.y;
    }
    ready = true;
    try {
      mon = await monitorArea();
    } catch {
      /* defaults */
    }
    // restore pinned state: pinned icons stay where they were, the rest
    // fall in from the top on arrival as before
    vy = 0;
    settled = rec.data["pinned"] === true;
    if (settled) wrap.classList.add("rest");

    window.setInterval(() => void saveSoon(), 4000);
    window.addEventListener("beforeunload", () => void saveSoon());
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) void saveSoon();
    });
    window.setInterval(async () => {
      try {
        mon = await monitorArea();
      } catch {
        /* keep */
      }
      try {
        const s = await appWin.scaleFactor();
        if (s > 0) scale = s;
      } catch {
        /* keep */
      }
    }, 2000);
    window.setInterval(async () => {
      try {
        const all = await invoke<LayoutItem[]>("floaty_layout");
        others = all.filter((o) => o.id !== id);
      } catch {
        /* keep */
      }
    }, 500);
  })();

  wrap.querySelector(".launcher-x")?.addEventListener("pointerdown", (e) => e.stopPropagation());
  wrap.querySelector(".launcher-x")?.addEventListener("click", (e) => {
    e.stopPropagation();
    if (rec) void removeSelf(rec);
  });

  // Manual drag (same reason as the pet): an OS-level startDragging on every
  // press swallows the click sequence, so dblclick-to-launch never fires.
  // The window follows the cursor and x/y stay exact — no re-read needed.
  wrap.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || dragging) return;
    e.stopPropagation();
    // NOTE: no preventDefault() — canceling pointerdown kills click/dblclick.
    dragging = true;
    wrap.classList.add("held");
    squash();
    try {
      wrap.setPointerCapture(e.pointerId);
    } catch {
      /* capture unsupported — window-level listeners still cover the drag */
    }
    const startX = x;
    const startY = y;
    const startSX = e.screenX;
    const startSY = e.screenY;
    let moved = false;
    const onMove = (ev: PointerEvent) => {
      x = startX + (ev.screenX - startSX);
      y = startY + (ev.screenY - startSY);
      if (x < mon.x) x = mon.x;
      if (x > mon.x + mon.w - WIN_W) x = mon.x + mon.w - WIN_W;
      if (y < mon.y) y = mon.y;
      if (y > mon.y + mon.h - WIN_H) y = mon.y + mon.h - WIN_H;
      if (Math.hypot(ev.screenX - startSX, ev.screenY - startSY) > 4) {
        moved = true;
        suppressClickUntil = performance.now() + 300;
      }
      void appWin
        .setPosition(new PhysicalPosition(Math.round(x * scale), Math.round(y * scale)))
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
      wrap.classList.remove("held");
      if (moved) {
        // was a real drag: pin exactly where dropped, no fall
        vx = 0;
        vy = 0;
        settled = true;
        wrap.classList.add("rest");
        void saveSoon();
      }
      // a tap leaves `settled` untouched — click/dblclick decide what happens
      dragging = false;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  });

  const doDrop = () => {
    settled = false;
    wrap.classList.remove("rest");
    vy = 0;
    vx += (Math.random() - 0.5) * 60;
    squash();
  };
  const doHop = () => {
    settled = false;
    wrap.classList.remove("rest");
    vy = -420;
    vx += (Math.random() - 0.5) * 120;
    squash();
  };
  const doPin = () => {
    vx = 0;
    vy = 0;
    settled = true;
    wrap.classList.add("rest");
    void saveSoon();
  };
  wrap.addEventListener("click", (e) => {
    e.stopPropagation();
    if (performance.now() < suppressClickUntil) return; // was a drag, not a tap
    // single-click action from settings (cancelled if a double-click follows)
    window.clearTimeout(clickTimer);
    clickTimer = window.setTimeout(() => {
      const a = currentSettings().single_click;
      if (a === "hop") doHop();
      else if (a === "nothing") { /* stay put */ }
      else doDrop();
    }, 260);
  });
  wrap.addEventListener("dblclick", (e) => {
    e.stopPropagation();
    window.clearTimeout(clickTimer);
    const a = currentSettings().double_click;
    if (a === "drop") {
      doDrop();
      return;
    }
    if (a === "nothing") {
      doPin();
      return;
    }
    if (!rec) return;
    // launch without dropping: pin in place even if it was falling
    doPin();
    tile.classList.add("launching");
    window.setTimeout(() => tile.classList.remove("launching"), 600);
    invoke("floaty_launch", { id: rec.id }).catch(() => {
      tile.classList.remove("launching");
      tile.classList.add("shake");
      window.setTimeout(() => tile.classList.remove("shake"), 500);
    });
  });
  wrap.addEventListener("contextmenu", (e) => e.preventDefault());

  const frame = (now: number) => {
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    // pinned icons skip physics entirely — they stay where dropped.
    // Never drive the window before the stored position loads (ready),
    // or every icon first jumps to default coordinates and bunches up.
    if (ready && !dragging && !settled && !document.hidden) {
      // gravity (live from settings)
      vy += currentSettings().gravity * dt;
      vx *= 1 - 0.12 * dt;
      x += vx * dt;
      y += vy * dt;

      // inter-icon: slide sideways out of overlap and rest on top of piles.
      // (The old radial push fought gravity into a mid-air hover equilibrium
      // that stalled drops, fed by stale ghost positions.)
      const cx = x + WIN_W / 2;
      for (const o of others) {
        const ox = o.x + o.w / 2;
        const overlapX = Math.min(x + WIN_W, o.x + o.w) - Math.max(x, o.x);
        const overlapY = Math.min(y + WIN_H, o.y + o.h) - Math.max(y, o.y);
        if (overlapX > 8 && overlapY > 8) {
          const dir = cx >= ox ? 1 : -1;
          const shove = Math.min(overlapX, 40);
          x += dir * shove * Math.min(1, 6 * dt);
          vx += dir * 260 * dt;
          // land on top when falling onto another icon
          const ourBottom = y + WIN_H;
          if (vy >= 0 && ourBottom >= o.y && ourBottom - o.y < 34) {
            y = o.y - WIN_H;
            if (Math.abs(vy) > 140) {
              vy = -vy * currentSettings().bounce;
              vx *= 0.9;
              squash();
            } else {
              vy = 0;
              vx *= 1 - Math.min(1, 4 * dt);
              if (Math.abs(vx) < 14) {
                vx = 0;
                if (!settled) {
                  settled = true;
                  wrap.classList.add("rest");
                  void saveSoon();
                }
              }
            }
          }
        }
      }

      // floor / walls / ceiling
      const floor = mon.y + mon.h - WIN_H - 6;
      if (y >= floor) {
        y = floor;
        if (Math.abs(vy) > 110) {
          vy = -vy * currentSettings().bounce;
          vx *= 0.9;
          squash();
        } else {
          vy = 0;
          vx *= 1 - Math.min(1, 4 * dt);
          if (Math.abs(vx) < 14) {
            vx = 0;
            if (!settled) {
              settled = true;
              wrap.classList.add("rest");
              void saveSoon();
            }
          }
        }
      }
      if (x < mon.x) {
        x = mon.x;
        vx = Math.abs(vx) * REST_WALL;
      }
      if (x > mon.x + mon.w - WIN_W) {
        x = mon.x + mon.w - WIN_W;
        vx = -Math.abs(vx) * REST_WALL;
      }
      if (y < mon.y) {
        y = mon.y;
        vy = Math.abs(vy) * 0.5;
      }

      if (!settled) {
        wrap.classList.remove("rest");
        void setLogicalPos(x, y).catch(() => undefined);
      }
    }
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
}
