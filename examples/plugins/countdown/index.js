// Countdown — an example floaty plugin.
//
// It only uses the documented surface: the module is imported by the overlay and
// handed `api` (see docs/plugins.md). No imports, no build step, no knowledge of
// floaty's internals.

const CSS = `
  .cd-wrap {
    width: 100%; height: 100%;
    display: flex; flex-direction: column; align-items: center; justify-content: center;
    gap: 4px; border-radius: 16px; padding: 10px; box-sizing: border-box;
    font-family: ui-sans-serif, system-ui, "Segoe UI", sans-serif;
    background: linear-gradient(160deg, rgba(34,32,66,.92), rgba(22,21,44,.94));
    color: #f4f3ff; border: 1px solid rgba(255,255,255,.12);
    box-shadow: var(--card-shadow, 0 5px 16px rgba(20,16,40,.2));
    overflow: hidden;
  }
  .cd-title { font-size: 11px; letter-spacing: .12em; text-transform: uppercase; opacity: .6; }
  .cd-time { font-size: 30px; font-weight: 650; font-variant-numeric: tabular-nums; line-height: 1; }
  .cd-sub { font-size: 11px; opacity: .65; text-align: center; }
  .cd-input { margin-top: 6px; background: rgba(255,255,255,.08); color: inherit;
    border: 1px solid rgba(255,255,255,.2); border-radius: 8px; padding: 3px 6px; font: inherit; font-size: 12px; }
`;

function pad(n) {
  return String(n).padStart(2, "0");
}

/** Break a millisecond span into the two units worth showing. */
function parts(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const days = Math.floor(s / 86400);
  const hours = Math.floor((s % 86400) / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  const seconds = s % 60;
  if (days > 0) return { main: `${days}d ${hours}h`, sub: `${minutes}m ${seconds}s left` };
  if (hours > 0) return { main: `${hours}:${pad(minutes)}:${pad(seconds)}`, sub: "hours left" };
  return { main: `${minutes}:${pad(seconds)}`, sub: "minutes left" };
}

export default {
  async mount(root, id, api) {
    const style = document.createElement("style");
    style.textContent = CSS;

    const wrap = document.createElement("div");
    wrap.className = "cd-wrap";
    const title = document.createElement("div");
    title.className = "cd-title";
    const time = document.createElement("div");
    time.className = "cd-time";
    const sub = document.createElement("div");
    sub.className = "cd-sub";
    wrap.append(title, time, sub);
    root.append(style, wrap);

    const rec = await api.record.load(id);
    if (!rec) {
      api.log(`countdown ${id}: record not found`);
      return;
    }

    // clicking the widget swaps the countdown for a date picker
    let editing = false;
    wrap.addEventListener("click", (ev) => {
      if (editing) return;
      ev.stopPropagation();
      editing = true;
      const input = document.createElement("input");
      input.type = "datetime-local";
      input.className = "cd-input";
      if (rec.data.target) input.value = String(rec.data.target).slice(0, 16);
      const commit = async () => {
        editing = false;
        rec.data.target = input.value || "";
        await api.record.save(rec);
        input.remove();
        render();
      };
      input.addEventListener("change", commit);
      input.addEventListener("blur", commit);
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") void commit();
      });
      wrap.append(input);
      input.focus();
    });

    function render() {
      const target = typeof rec.data.target === "string" ? rec.data.target : "";
      if (!target) {
        title.textContent = "countdown";
        time.textContent = "--:--";
        sub.textContent = "click to pick a date";
        wrap.title = "click to pick a date";
        return;
      }
      const when = new Date(target).getTime();
      const left = when - Date.now();
      const { main, sub: label } = parts(left);
      title.textContent = left > 0 ? "time left" : "elapsed";
      time.textContent = main;
      sub.textContent = left > 0 ? label : `since ${target.replace("T", " ")}`;
      wrap.title = `target ${target.replace("T", " ")}`;
    }

    render();
    api.log(`countdown ${id}: mounted, target=${rec.data.target || "none"} showing "${time.textContent}"`);
    const timer = window.setInterval(render, 1000);
    // the overlay may unmount the widget without telling the module; stop the
    // timer once the DOM is gone so a stale widget costs nothing
    const reaper = window.setInterval(() => {
      if (!wrap.isConnected) {
        window.clearInterval(timer);
        window.clearInterval(reaper);
      }
    }, 5000);

    // the standard floaty affordances: drag it, right-click it, resize it.
    // enableDrag is threshold-based, so the tap-to-edit above still works.
    api.enableDrag(wrap, rec);
    api.addPinMenu(wrap, () => rec);
    api.addResizeHandle(wrap, rec, 170, 96);
  },

  describe(rec) {
    const target = rec.data && typeof rec.data.target === "string" ? rec.data.target : "";
    return target ? `to ${target.replace("T", " ")}` : "no date set";
  },
};
