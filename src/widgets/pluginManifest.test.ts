import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { loadPluginManifest, minSizeFor } from "./pluginManifest";

/** One payload entry as the backend serves it; only the fields under test are meaningful. */
function entry(id: string, min: [number, number], max: [number, number]) {
  return {
    id,
    name: id,
    description: "",
    enabled: true,
    default_size: [300, 300],
    min_size: min,
    max_size: max,
    resizable: true,
    desktop_item: null,
    add_label: null,
    layout_priority: 5,
    source: "builtin",
    entry: null,
    version: "",
    author: "",
    api_version: 2,
    trusted: true,
  };
}

describe("minSizeFor", () => {
  beforeEach(() => {
    // Deliberately not the numbers any widget module used to hardcode: if the
    // grip still read a constant, these would not come back.
    vi.mocked(invoke).mockResolvedValue([
      entry("visualizer", [141, 73], [1400, 900]),
      entry("note", [333, 222], [1400, 1400]),
    ]);
  });

  it("returns the floor the backend sent, not a constant", async () => {
    await loadPluginManifest();
    expect(minSizeFor("visualizer")).toEqual({ w: 141, h: 73 });
    expect(minSizeFor("note")).toEqual({ w: 333, h: 222 });
  });

  it("falls back to the backend's own default floor for an unknown kind", async () => {
    await loadPluginManifest();
    expect(minSizeFor("no-such-kind")).toEqual({ w: 80, h: 60 });
  });
});
