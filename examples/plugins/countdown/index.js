// Countdown — an example floaty plugin.
//
// It only uses the documented surface: the module is imported by the overlay and
// handed `api` (see docs/plugins.md). No imports, no build step, no knowledge of
// floaty's internals.

const CSS = `
  .cd-wrap {
    position: relative;
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
  /* What it is counting down for: the widest of the two fields, and centred like the rest */
  .cd-what { width: 100%; text-align: center; }
  /* Two fields need the room the "minutes left" line was using */
  .cd-wrap.cd-editing .cd-sub { display: none; }
  .cd-title { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  /* The close button every floaty widget has: quiet until the pointer is over the widget,
     and a real <button> so the drag leaves it alone (floaty skips controls). */
  .cd-x {
    position: absolute; top: 5px; right: 5px; width: 22px; height: 22px; z-index: 3;
    border: none; border-radius: 50%; background: rgba(255,255,255,.12); color: #f4f3ff;
    font-size: 14px; line-height: 1; cursor: pointer; opacity: 0;
    transition: opacity .15s, background .15s, transform .15s;
  }
  .cd-wrap:hover .cd-x { opacity: .6; }
  .cd-x:hover { opacity: 1 !important; background: rgba(255,255,255,.26); transform: scale(1.08); }
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
    const close = document.createElement("button");
    close.className = "cd-x";
    close.type = "button";
    close.title = "Remove";
    close.textContent = "\u00d7";
    const title = document.createElement("div");
    title.className = "cd-title";
    const time = document.createElement("div");
    time.className = "cd-time";
    const sub = document.createElement("div");
    sub.className = "cd-sub";
    wrap.append(title, time, sub, close);
    root.append(style, wrap);

    const rec = await api.record.load(id);
    if (!rec) {
      api.log(`countdown ${id}: record not found`);
      return;
    }

    // The close button removes the widget — the same thing the right-click menu's
    // "remove" does, and what every built-in widget offers in its corner.
    close.addEventListener("click", (ev) => {
      ev.stopPropagation();
      void api.removeSelf(rec);
    });
    // ...and it must not start a drag on its way there.
    close.addEventListener("pointerdown", (ev) => ev.stopPropagation());

    // clicking the widget swaps the countdown for its two fields: what it is for, and when
    let editing = false;
    wrap.addEventListener("click", (ev) => {
      if (editing) return;
      ev.stopPropagation();
      editing = true;
      const beforeWhat = typeof rec.data.what === "string" ? rec.data.what : "";
      const before = typeof rec.data.target === "string" ? rec.data.target : "";

      const what = document.createElement("input");
      what.type = "text";
      what.className = "cd-input cd-what";
      what.placeholder = "counting down for…";
      what.maxLength = 48;
      what.value = beforeWhat;

      const input = document.createElement("input");
      input.type = "datetime-local";
      input.className = "cd-input";
      if (before) input.value = before.slice(0, 16);

      // A whole datetime, or nothing: while a segment is being edited Chromium
      // reports the value with that segment blanked ("2026--20T10:00"), and an
      // empty or half-typed value must never become the target.
      const complete = (v) => /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2})?$/.test(v);

      // Editing applies the value but keeps the input open. Chromium fires
      // `change` on the *first* segment edit when the field already held a
      // complete value, so treating `change` as "done" removed the input under
      // the first keystroke (and stored a blank target). The input is only torn
      // down when the user leaves it: Enter, or clicking away (blur).
      // The date is applied as it is edited (see the note above about half-typed
      // segments); the name is applied as it is typed but saved when the editor closes,
      // because a save per keystroke would rewrite the whole store for every letter.
      const apply = async () => {
        if (!editing || !complete(input.value) || input.value === rec.data.target) return;
        rec.data.target = input.value;
        await api.record.save(rec);
        render();
      };
      const finish = async (cancel) => {
        if (!editing) return;
        editing = false;
        if (cancel) {
          rec.data.what = beforeWhat;
          rec.data.target = before;
        } else {
          rec.data.what = what.value.trim();
          if (!input.value) rec.data.target = ""; // cleared on purpose
          else if (complete(input.value)) rec.data.target = input.value;
        }
        await api.record.save(rec);
        what.remove();
        input.remove();
        wrap.classList.remove("cd-editing");
        render();
      };
      const leaving = () => {
        // Moving between the two fields is not leaving the editor: the blur fires before
        // focus lands, so the check has to wait a tick and see where it went.
        window.setTimeout(() => {
          if (!editing) return;
          const active = document.activeElement;
          if (active === what || active === input) return;
          void finish(false);
        }, 0);
      };
      for (const field of [what, input]) {
        field.addEventListener("input", () => {
          if (field === what) {
            rec.data.what = what.value.trim();
            render(); // the title follows the typing; the save waits for the close
          } else {
            void apply();
          }
        });
        field.addEventListener("change", () => void apply());
        field.addEventListener("blur", leaving);
        field.addEventListener("keydown", (e) => {
          if (e.key === "Enter") void finish(false);
          if (e.key === "Escape") void finish(true);
        });
      }
      wrap.classList.add("cd-editing");
      wrap.append(what, input);
      what.focus();
    });

    function render() {
      // The title is the user's own words — what the countdown is *for* — so it says
      // "countdown" only until they name it. How much time is left is the sub line's job,
      // which keeps "time left" and "elapsed" without competing for the same row.
      const what = typeof rec.data.what === "string" ? rec.data.what : "";
      const target = typeof rec.data.target === "string" ? rec.data.target : "";
      title.textContent = what || "countdown";
      if (!target) {
        time.textContent = "--:--";
        sub.textContent = what ? "click to pick a date" : "click to name it and pick a date";
        wrap.title = "click to name it and pick a date";
        return;
      }
      const when = new Date(target).getTime();
      const left = when - Date.now();
      const { main, sub: label } = parts(left);
      time.textContent = main;
      sub.textContent = left > 0 ? label : `since ${target.replace("T", " ")}`;
      wrap.title = `${what ? `${what} — ` : ""}target ${target.replace("T", " ")}`;
    }

    render();
    api.log(`countdown ${id}: mounted, what=${rec.data.what || "none"}, target=${rec.data.target || "none"} showing "${time.textContent}"`);
    // A timer floaty owns: it is cleared when the widget goes, so there is no reaper to write
    // and nothing to leak if the overlay unmounts this without telling the module.
    api.every(id, 1000, render);

    // the standard floaty affordances: drag it, right-click it, resize it.
    // enableDrag is threshold-based, so the tap-to-edit above still works.
    api.enableDrag(wrap, rec);
    api.addPinMenu(wrap, () => rec);
    api.addResizeHandle(wrap, rec, 170, 96);
  },

  describe(rec) {
    const what = rec.data && typeof rec.data.what === "string" ? rec.data.what : "";
    const target = rec.data && typeof rec.data.target === "string" ? rec.data.target : "";
    if (what && target) return `${what} — to ${target.replace("T", " ")}`;
    if (what) return what;
    return target ? `to ${target.replace("T", " ")}` : "no date set";
  },
};
