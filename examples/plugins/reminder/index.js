// Reminder — an example floaty plugin, and the apiVersion 2 tour.
//
// It needs "apiVersion": 2 in plugin.json, and that is not a formality: the four
// things it leans on are the v2 half of the widget api — api.notify (say something
// the user will notice from anywhere), api.every and api.after (timers the *widget*
// owns, so floaty stops them when it goes) and api.on(id, "display", …) (the screen
// went off, or came back). A manifest that says 1 still loads, with exactly the old
// surface: calling any of these from one throws
// 'api.notify needs "apiVersion": 2 in plugin.json', which is more use than
// "api.notify is not a function".
//
// What it is: three wall-clock reminders, the next one on the card, and a Windows
// notification when one comes due. The quiet button in its corner sits the rest of
// the day out.
//
// The honest limit, worth copying along with the code: a plugin only runs while
// floaty runs, so this is not a scheduler and cannot wake a closed app. What it
// does instead is catch up — at mount, and when the screen comes back on — for
// whatever came due while nobody was looking.

const CSS = `
  .rm-wrap {
    width: 100%; height: 100%;
    display: flex; flex-direction: column; justify-content: center;
    gap: 3px; padding: 9px 14px; box-sizing: border-box; border-radius: 16px;
    font-family: ui-sans-serif, system-ui, "Segoe UI", sans-serif;
    background: linear-gradient(160deg, rgba(34,32,66,.92), rgba(22,21,44,.94));
    color: #f4f3ff; border: 1px solid rgba(255,255,255,.12);
    box-shadow: var(--card-shadow, 0 5px 16px rgba(20,16,40,.2));
    overflow: hidden;
  }
  .rm-top { display: flex; align-items: center; gap: 6px; font-size: 10px;
    letter-spacing: .12em; text-transform: uppercase; opacity: .6; }
  .rm-flag { margin-left: auto; }
  .rm-quiet { border: 0; background: none; color: inherit; font: inherit; font-size: 10px;
    letter-spacing: .12em; text-transform: inherit; padding: 1px 4px; border-radius: 6px; cursor: pointer; }
  .rm-quiet:hover { background: rgba(255,255,255,.12); }
  .rm-time { font-size: 26px; font-weight: 650; font-variant-numeric: tabular-nums; line-height: 1.05; }
  .rm-what { font-size: 12px; opacity: .82; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .rm-when { font-size: 11px; opacity: .58; }
`;

/**
 * The reminders, as shipped. Once the widget exists they live in its own record
 * (`rec.data.times`), which is where the days each one was met are kept too — so a
 * restart brings back what the user changed without a settings file of its own.
 * Editing them is a matter of writing that list back; three shows the shape.
 */
const TIMES = [
  { at: "09:30", what: "stand up and stretch" },
  { at: "13:00", what: "water the plants" },
  { at: "17:30", what: "write down what is left for tomorrow" },
];

/** A reminder is a minute-resolution thing: nothing here needs a one-second clock. */
const TICK_MS = 15000;
/**
 * The one-shot after mount. The first tick is a quarter of a minute away, and a
 * reminder that came due in the seconds around a launch should be raised with the
 * card already on screen to say what is next.
 */
const FIRST_CHECK_MS = 2000;
/**
 * How late a reminder can be and still be worth announcing. Past this it is marked
 * met in silence: floaty was closed, the machine was asleep, the screen was off for
 * the morning — "water the plants" three hours late is noise, not news.
 */
const GRACE_MS = 90 * 60 * 1000;
/** Under this, a notification reads as "now" and gets no "was due at" line. */
const LATE_MS = 90 * 1000;

const pad = (n) => String(n).padStart(2, "0");

/** The day a Date falls on, in the user's own timezone: the unit "met" is kept in. */
const dayKey = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

/** How far off a reminder is, in the two units a 260px card has room for. */
function until(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `in ${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `in ${m}m`;
  return `in ${Math.floor(m / 60)}h ${m % 60}m`;
}

/** The moment `at` ("HH:MM") falls on the day `day` is in. */
function occurrence(at, day) {
  const [h, m] = String(at).split(":").map(Number);
  const hours = Number.isFinite(h) ? h : 0;
  const minutes = Number.isFinite(m) ? m : 0;
  return new Date(day.getFullYear(), day.getMonth(), day.getDate(), hours, minutes, 0, 0);
}

/** A record's reminder list, in the order the day runs in. */
function timesOf(rec) {
  const list = rec.data && Array.isArray(rec.data.times) ? rec.data.times : [];
  return list.slice().sort((a, b) => String(a.at).localeCompare(String(b.at)));
}

export default {
  async mount(root, id, api) {
    const style = document.createElement("style");
    style.textContent = CSS;

    const wrap = document.createElement("div");
    wrap.className = "rm-wrap";
    const top = document.createElement("div");
    top.className = "rm-top";
    const label = document.createElement("span");
    label.textContent = "reminder";
    const flag = document.createElement("span");
    flag.className = "rm-flag";
    const quietBtn = document.createElement("button");
    quietBtn.type = "button";
    quietBtn.className = "rm-quiet";
    top.append(label, flag, quietBtn);
    const time = document.createElement("div");
    time.className = "rm-time";
    const what = document.createElement("div");
    what.className = "rm-what";
    const when = document.createElement("div");
    when.className = "rm-when";
    wrap.append(top, time, what, when);
    root.append(style, wrap);

    const rec = await api.record.load(id);
    if (!rec) {
      api.log(`reminder ${id}: record not found`);
      return;
    }

    // The list is seeded once, into the record, and from then on the record is the
    // truth: what the user changed, and the days each time was met, come back after
    // a restart.
    if (timesOf(rec).length === 0) {
      rec.data.times = TIMES.map((entry) => ({ at: entry.at, what: entry.what }));
    }
    // "met" is the day each time was last raised, keyed by the time — unique in this
    // list, and robust to the list being reordered. It is the *whole* guard against
    // firing twice: the tick below matches the same reminder every 15 seconds for
    // the rest of the day, and this map is what makes the second match a no-op.
    if (!rec.data.met || typeof rec.data.met !== "object" || Array.isArray(rec.data.met)) {
      rec.data.met = {};
    }
    if (typeof rec.data.quiet !== "string") rec.data.quiet = "";
    await api.record.save(rec);

    const met = () => rec.data.met;
    const isQuiet = () => rec.data.quiet === dayKey(new Date());

    // A notification raised at a dark screen is gone by morning. While the display
    // is off, reminders are held rather than raised — nothing is marked met, so the
    // pass on the way back raises exactly what was missed.
    let screenOff = false;
    let stopDisplay = null;

    function render() {
      const now = new Date();
      const list = timesOf(rec);
      // The next one, which may be tomorrow's first: the card keeps saying what is
      // coming rather than going blank once today's last reminder has passed.
      let next = list.find((entry) => occurrence(entry.at, now).getTime() > now.getTime()) || null;
      let tomorrow = false;
      if (!next && list.length > 0) {
        next = list[0];
        tomorrow = true;
      }

      flag.textContent = screenOff ? "screen off" : "";
      quietBtn.textContent = isQuiet() ? "listen" : "quiet";

      if (!next) {
        time.textContent = "--:--";
        what.textContent = "no reminders set";
        when.textContent = "";
        wrap.title = "no reminders set";
        return;
      }

      const at = tomorrow ? occurrence(next.at, new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1)) : occurrence(next.at, now);
      time.textContent = next.at;
      // api.displayName is the desktop's caption rule (Arc.lnk reads as Arc), so a
      // reminder named after a shortcut reads like the icons it stands next to. It
      // leaves ordinary text alone, which is most of them.
      what.textContent = api.displayName(String(next.what || ""));
      when.textContent = `${tomorrow ? "tomorrow · " : ""}${until(at.getTime() - now.getTime())}`;
      wrap.title = `${next.at} ${next.what}${tomorrow ? " (tomorrow)" : ""}`;
    }

    /**
     * Raise everything that has come due, and mark it met — once per occurrence,
     * never repeatedly. Silent for anything past the grace window, because a
     * reminder the user has already lived through is not worth a notification.
     */
    async function checkDue() {
      if (screenOff) return; // held: raised when the display comes back
      const now = new Date();
      const today = dayKey(now);
      let changed = false;
      for (const entry of timesOf(rec)) {
        const key = String(entry.at);
        const at = occurrence(key, now);
        if (at.getTime() > now.getTime()) continue; // still to come
        if (met()[key] === today) continue; // already raised today
        const late = now.getTime() - at.getTime();
        met()[key] = today;
        changed = true;
        if (late > GRACE_MS) {
          // Marked met anyway, so it cannot surface later as a stale notification —
          // and said out loud in the log, which is the only place this shows.
          api.log(`reminder ${id}: ${key} ${entry.what} missed by ${Math.round(late / 60000)}m — marked met without a notification`);
          continue;
        }
        // notify needs no permission of its own (it is raised in Rust), which is why
        // it is the only way a widget gets through a covered desktop.
        await api.notify(
          `reminder: ${api.displayName(String(entry.what || ""))}`,
          late > LATE_MS ? `was due at ${key}` : `it is ${key}`,
        );
        api.log(`reminder ${id}: raised ${key} ${entry.what} (${Math.round(late / 1000)}s late)`);
      }
      if (changed) await api.record.save(rec);
    }

    /**
     * Follow the display. The listener is only worth having while reminders could
     * be raised: a quiet day has nothing to catch up on, so it is dropped for the
     * day (unfollowDisplay below) and taken up again by the tick — which is what
     * the function `api.on` returns is for. floaty would clear it on removal anyway.
     */
    function followDisplay() {
      if (stopDisplay) return;
      stopDisplay = api.on(id, "display", onDisplay);
    }

    function unfollowDisplay() {
      if (!stopDisplay) return;
      stopDisplay();
      stopDisplay = null;
    }

    function onDisplay(payload) {
      // The payload is { display: "on" | "off" }. Anything else is a shape this
      // plugin does not know, and guessing "on" would raise notifications at a
      // screen nobody can see, so it is ignored.
      const state = payload && typeof payload === "object" ? payload.display : undefined;
      if (state !== "on" && state !== "off") return;
      screenOff = state === "off";
      api.log(`reminder ${id}: display ${state}${screenOff ? " — holding reminders" : " — catching up"}`);
      render();
      if (!screenOff) void checkDue();
    }

    async function setQuiet(on) {
      rec.data.quiet = on ? dayKey(new Date()) : "";
      await api.record.save(rec);
      if (on) unfollowDisplay();
      else followDisplay();
      api.log(`reminder ${id}: ${on ? "quiet for the rest of today" : "listening again"}`);
      render();
    }

    /** One pass: repaint the countdown, raise anything that has come due. */
    function tick() {
      // a quiet day ends by itself — the stored date stops being today, and the
      // display listener is wanted again. Saving it keeps the record saying what the
      // widget is doing, for the settings list and the next mount.
      if (rec.data.quiet && !isQuiet()) {
        rec.data.quiet = "";
        followDisplay();
        void api.record.save(rec);
      }
      render();
      void checkDue();
    }

    render();
    api.log(`reminder ${id}: mounted on api v${api.apiVersion}, ${timesOf(rec).length} reminder(s)`);

    // Both timers belong to the widget rather than to the module: floaty clears them
    // when the widget is removed or remounted, so there is no reaper interval to
    // write here — which is the whole reason to reach for api.every and api.after
    // instead of window.setInterval.
    api.every(id, TICK_MS, tick);
    api.after(id, FIRST_CHECK_MS, tick);

    if (!isQuiet()) followDisplay();

    quietBtn.addEventListener("click", (ev) => {
      ev.stopPropagation(); // a click on the button is not a click on the card
      void setQuiet(!isQuiet());
    });

    // the standard floaty affordances: drag it, right-click it for the usual rows.
    // enableDrag is threshold-based, so a press on the quiet button is still a click.
    api.enableDrag(wrap, rec);
    api.addPinMenu(wrap, () => rec);
  },

  describe(rec) {
    const list = timesOf(rec);
    if (list.length === 0) return "no reminders set";
    const now = new Date();
    const next = list.find((entry) => occurrence(entry.at, now).getTime() > now.getTime());
    if (!next) return `all met — next ${list[0].at} tomorrow`;
    return `next ${next.at} · ${next.what}`;
  },
};
