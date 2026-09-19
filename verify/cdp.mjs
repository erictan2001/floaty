/**
 * A very small Chrome DevTools Protocol client, for the verification scripts in
 * this folder.
 *
 * No dependencies on purpose: Node 21+ has a global `WebSocket`, and everything
 * here needs is a target list, one method call at a time, and a key event. The
 * alternative — puppeteer, playwright — is 100MB of browser plus a second copy of
 * the browser we are actually testing (WebView2), for the same three calls.
 *
 * The two things worth knowing about CDP here:
 *
 * - **`Runtime.evaluate` with `awaitPromise` is how a backend command gets run.**
 *   The page has `window.__TAURI_INTERNALS__.invoke`, so a probe talks to the real
 *   store through the real IPC, with no test seam inside the app.
 * - **A page that reloads takes the evaluation with it.** `floaty_install_plugin`
 *   and `floaty_rescan_plugins` reload every window; a probe that awaits them gets
 *   `null` instead of an error. Call those un-awaited and read the outcome from
 *   the log and from disk.
 */

import { spawn } from "node:child_process";
import fs from "node:fs";
import { createServer } from "node:net";
import os from "node:os";
import path from "node:path";

/** The port `launchChrome` picks when a probe needs a browser of its own. */
export const DEFAULT_PORT = 9333;
/**
 * The port the *app's* pages are on — the app is started with
 * `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"`. Probes that
 * drive the running app must use this one; probes that drive their own Chrome use
 * `DEFAULT_PORT`. Mixing them up fails as "no running app", which is what happened.
 */
export const APP_PORT = 9222;

/**
 * Console noise that is nobody's fault and would otherwise fail every run: a
 * missing favicon is the whole list so far. Anything else is reported with its url,
 * so a real 404 is not hidden by this.
 */
export const NOISE = [/favicon\.ico/];

/** The entries of `errors` worth reporting. */
export function realErrors(errors) {
  return errors.filter((e) => !NOISE.some((pattern) => pattern.test(e)));
}

/** Targets on a running debug port, newest first as the browser lists them. */
export async function listTargets(port = DEFAULT_PORT) {
  const response = await fetch(`http://127.0.0.1:${port}/json/list`);
  return await response.json();
}

/** The page whose url ends with `suffix` — `#/overlay`, `settings.html#/general`, … */
/**
 * The first page whose url ends with `suffix`.
 *
 * `nth` picks among several matches, which a multi-monitor desktop needs: every
 * overlay window is `index.html#/overlay`, so "#/overlay" alone is ambiguous once
 * there is a second screen.
 */
export async function findTarget(suffix, port = DEFAULT_PORT, nth = 0) {
  const targets = await listTargets(port);
  const matches = targets.filter((t) => t.type === "page" && t.url.endsWith(suffix));
  const hit = matches[nth];
  if (!hit) {
    throw new Error(
      `no page ending "${suffix}" (number ${nth + 1} of ${matches.length}) on port ${port}; open targets: ` +
        targets
          .filter((t) => t.type === "page")
          .map((t) => t.url)
          .join(", "),
    );
  }
  return hit;
}

export class Session {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    /** console errors and exceptions seen since connect, newest last */
    this.errors = [];
  }

  static async open(port = DEFAULT_PORT, suffix = "#/overlay", nth = 0) {
    const target = await findTarget(suffix, port, nth);
    const socket = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", reject, { once: true });
    });
    const session = new Session(socket);
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id && session.pending.has(message.id)) {
        session.pending.get(message.id)(message);
        session.pending.delete(message.id);
        return;
      }
      // the two events that mean "the page complained", which no DOM check sees
      if (message.method === "Runtime.exceptionThrown") {
        const d = message.params?.exceptionDetails ?? {};
        session.errors.push(`exception: ${d.text ?? ""} ${d.exception?.description ?? ""}`.trim());
      }
      if (message.method === "Runtime.consoleAPICalled" && message.params?.type === "error") {
        session.errors.push(
          `console.error: ${(message.params.args ?? [])
            .map((a) => a.value ?? a.description ?? "")
            .join(" ")}`,
        );
      }
      if (message.method === "Log.entryAdded" && message.params?.entry?.level === "error") {
        const entry = message.params.entry;
        // the url matters: "Failed to load resource" is a 404 for `favicon.ico`
        // as often as for something the app actually needs
        session.errors.push(`log: ${entry.text}${entry.url ? ` (${entry.url})` : ""}`);
      }
    });
    await session.send("Runtime.enable");
    await session.send("Log.enable");
    await session.send("Page.enable");
    return session;
  }

  send(method, params = {}) {
    const id = this.nextId++;
    this.socket.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve) => this.pending.set(id, resolve)).then((message) => {
      if (message.error) throw new Error(`${method}: ${JSON.stringify(message.error)}`);
      return message.result;
    });
  }

  /**
   * Run an expression in the page and return its value.
   *
   * The expression is wrapped in an async function, so `await …` works as written
   * — `npm run verify:app -- js "await invoke('floaty_undo_state')"` is the shape
   * anyone reaches for first, and evaluating it bare made the page throw
   * "await is only valid in async functions". The newlines keep a trailing `//`
   * comment from swallowing the closing bracket.
   */
  async evaluate(expression, { awaitPromise = true } = {}) {
    const evaluate = (text) =>
      this.send("Runtime.evaluate", {
        expression: text,
        awaitPromise,
        returnByValue: true,
        userGesture: true,
      });

    // An expression first (`await invoke(...)`, `document.title`), and a statement
    // list if that does not parse (`await x; return y`) — the one shape anyone
    // types should not need a wrapper explained to them.
    let result = await evaluate(`(async () => (\n${expression}\n))()`);
    if (result.exceptionDetails?.exception?.description?.includes("SyntaxError")) {
      result = await evaluate(`(async () => {\n${expression}\n})()`);
    }
    if (result.exceptionDetails) {
      // A rejected promise arrives as "Uncaught (in promise)" with the reason
      // somewhere in the object, and a bare "evaluate threw: Uncaught (in promise)"
      // is useless for the one thing this is for: finding out why. Dump what there
      // is instead of guessing which field holds it.
      const exception = result.exceptionDetails.exception ?? {};
      const reason = exception.description || result.exceptionDetails.text;
      const detail =
        reason === "Uncaught (in promise)"
          ? JSON.stringify(exception.value ?? exception.preview ?? exception, null, 2)
          : "";
      throw new Error(`evaluate threw: ${reason}${detail ? `\n${detail}` : ""}`);
    }
    return result.result?.value;
  }

  /** Invoke a backend command from the page — the real one, through the real IPC. */
  invoke(command, args = {}) {
    return this.evaluate(
      `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
    );
  }

  /**
   * A key press that is a *real* key: `Input.dispatchKeyEvent` goes through the
   * same input pipeline as the user's keyboard, so a handler that needs a default
   * action (or that Blink gates on a trusted event) sees one. A synthetic
   * `dispatchEvent(new KeyboardEvent(...))` in the page does not.
   */
  async key(combo) {
    const { key, code, vk, modifiers } = parseKey(combo);
    // `text` is the character the key *produces*, and Chrome rejects anything that
    // is not one: sending the key's name (`"arrowdown"`) for a named key fails the
    // whole call with "Invalid 'text' parameter". Named keys are described by `key`
    // and `code` alone, and a modified chord produces nothing.
    const text = !modifiers && [...key].length === 1 ? key : "";
    for (const type of ["keyDown", "keyUp"]) {
      await this.send("Input.dispatchKeyEvent", {
        type,
        key,
        code,
        windowsVirtualKeyCode: vk,
        nativeVirtualKeyCode: vk,
        text: type === "keyDown" ? text : "",
        modifiers,
      });
    }
  }

  async screenshot(file) {
    const { data } = await this.send("Page.captureScreenshot", { format: "png" });
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, Buffer.from(data, "base64"));
    return file;
  }

  /**
   * Close the socket and give the event loop a moment to finish the handshake.
   *
   * Exiting straight after `close()` trips a libuv assertion on Windows
   * (`!(handle->flags & UV_HANDLE_CLOSING)`), which prints a crash on the way out
   * of a run that passed — the kind of noise that teaches people to ignore output.
   */
  async close() {
    try {
      this.socket.close();
    } catch {
      /* already gone */
    }
    await new Promise((resolve) => setTimeout(resolve, 60));
  }
}

/** `ctrl+shift+z` -> the CDP shape. Modifier bits: alt 1, ctrl 2, meta 4, shift 8. */
export function parseKey(combo) {
  const parts = combo.toLowerCase().split("+");
  const name = parts.pop();
  let modifiers = 0;
  for (const part of parts) {
    if (part === "alt") modifiers |= 1;
    if (part === "ctrl" || part === "control") modifiers |= 2;
    if (part === "meta" || part === "cmd") modifiers |= 4;
    if (part === "shift") modifiers |= 8;
  }
  const named = {
    escape: { key: "Escape", code: "Escape", vk: 27 },
    enter: { key: "Enter", code: "Enter", vk: 13 },
    tab: { key: "Tab", code: "Tab", vk: 9 },
    arrowdown: { key: "ArrowDown", code: "ArrowDown", vk: 40 },
    arrowup: { key: "ArrowUp", code: "ArrowUp", vk: 38 },
    space: { key: " ", code: "Space", vk: 32 },
    backspace: { key: "Backspace", code: "Backspace", vk: 8 },
  };
  if (named[name]) return { ...named[name], modifiers };
  const letter = name.length === 1 ? name : "";
  if (!letter) throw new Error(`verify: no key named "${combo}"`);
  return {
    key: letter,
    code: `Key${letter.toUpperCase()}`,
    vk: letter.toUpperCase().charCodeAt(0),
    modifiers,
  };
}

/**
 * A headless Chrome of our own, for the pages that do not need the app: the
 * settings window and a single widget page can both be loaded from the Vite dev
 * server with the Tauri IPC stubbed, which is the only way to look at them
 * without a second floaty instance fighting over the store.
 *
 * `inject` runs *before* any page script, which is where the stub has to land.
 */
/**
 * A port nobody is using.
 *
 * Not a fixed one: two probes must be able to run at the same time (a person
 * checking panes while another checks the palette), and a fixed port means the
 * second one silently drives the *first one's* browser — which reads as a probe
 * that passes for no reason, or a connect error that looks like a broken harness.
 */
async function freePort(start, tries = 24) {
  for (let port = start; port < start + tries; port++) {
    const free = await new Promise((resolve) => {
      const probe = createServer();
      probe.once("error", () => resolve(false));
      probe.once("listening", () => probe.close(() => resolve(true)));
      probe.listen(port, "127.0.0.1");
    });
    if (free) return port;
  }
  throw new Error(`verify: no free debug port from ${start}`);
}

export async function launchChrome({ port, window = "520,900", inject } = {}) {
  port ??= await freePort(DEFAULT_PORT);
  // A profile per process: Chrome holds handles on its profile for a moment after
  // it is killed, so the folder is left for the temp cleaner rather than deleted
  // here (an EPERM on cleanup must not take a passing run down with it).
  const profile = path.join(os.tmpdir(), `floaty-verify-chrome-${process.pid}`);
  try {
    fs.rmSync(profile, { recursive: true, force: true });
  } catch {
    /* a previous run's folder is still open; Chrome will reuse it */
  }
  const candidates = [
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    process.env.CHROME_PATH ?? "",
  ].filter(Boolean);
  const chrome = candidates.find((p) => fs.existsSync(p));
  if (!chrome) throw new Error("verify: no chrome.exe found (set CHROME_PATH)");

  const child = spawn(
    chrome,
    [
      "--headless=new",
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${profile}`,
      `--window-size=${window}`,
      "--no-first-run",
      "--no-default-browser-check",
      "about:blank",
    ],
    { stdio: "ignore" },
  );

  const deadline = Date.now() + 20_000;
  let ready = false;
  while (Date.now() < deadline && !ready) {
    try {
      await listTargets(port);
      ready = true;
    } catch {
      await new Promise((r) => setTimeout(r, 200));
    }
  }
  if (!ready) {
    child.kill();
    throw new Error("verify: chrome never opened its debug port");
  }

  const session = await Session.open(port, "about:blank");
  if (inject) await session.send("Page.addScriptToEvaluateOnNewDocument", { source: inject });
  return {
    session,
    port,
    port,
    stop() {
      try {
        session.close();
      } catch {
        /* already gone */
      }
      child.kill();
      try {
        fs.rmSync(profile, { recursive: true, force: true });
      } catch {
        /* Chrome still holds it; the temp folder is not worth a failed run */
      }
    },
  };
}
