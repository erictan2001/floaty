import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    label: "desktop-overlay",
    setSkipTaskbar: () => Promise.resolve(),
  }),
}));

if (typeof window === "undefined") {
  (globalThis as unknown as { window: unknown }).window = {
    location: { hash: "", pathname: "" },
    dispatchEvent: () => true,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  };
}

import { invoke } from "@tauri-apps/api/core";
import {
  ALLOWED_PLUGIN_COMMANDS,
  createScopedPluginApi,
  pluginApi,
  widgetApi,
  WidgetApi,
} from "./plugin";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string, args?: unknown) => Promise.resolve({ cmd, args })),
  convertFileSrc: (p: string) => `asset://${p}`,
}));

describe("Plugin Sandboxing and Scoped API", () => {
  it("hands full widgetApi to built-in plugins", () => {
    const api = pluginApi("note");
    expect(api).toBe(widgetApi);
  });

  it("hands a sandboxed API to third-party plugins", () => {
    const api = pluginApi("custom-third-party-widget");
    expect(api).not.toBe(widgetApi);
    expect(typeof api.invoke).toBe("function");
  });

  it("permits allowed safe commands through scoped API", async () => {
    const mockBase: WidgetApi = {
      ...widgetApi,
      invoke: vi.fn((cmd: string) =>
        Promise.resolve(`result-of-${cmd}`),
      ) as typeof widgetApi.invoke,
    };
    const scoped = createScopedPluginApi("countdown", mockBase);

    for (const cmd of Array.from(ALLOWED_PLUGIN_COMMANDS)) {
      const res = await scoped.invoke(cmd);
      expect(res).toBe(`result-of-${cmd}`);
    }
  });

  it("blocks dangerous commands with a security error", async () => {
    const mockBase: WidgetApi = {
      ...widgetApi,
      invoke: vi.fn().mockResolvedValue(undefined) as typeof widgetApi.invoke,
    };
    const scoped = createScopedPluginApi("malicious-plugin", mockBase);

    const forbidden = ["floaty_delete", "floaty_install_plugin", "shell_execute", "evil_cmd"];

    for (const cmd of forbidden) {
      await expect(scoped.invoke(cmd)).rejects.toThrow(
        `[security] Plugin "malicious-plugin" is not permitted to invoke command "${cmd}"`,
      );
    }
  });

  it("prefixes plugin logs with the plugin identifier", () => {
    const scoped = createScopedPluginApi("sample-widget", widgetApi);
    scoped.log("hello world");
    expect(invoke).toHaveBeenCalledWith("floaty_log", {
      msg: "[plugin:sample-widget] hello world",
    });
  });
});
