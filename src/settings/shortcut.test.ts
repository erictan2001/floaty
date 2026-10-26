import { describe, expect, it } from "vitest";
import { acceleratorFrom, isModifierCode, prettyAccelerator } from "./shortcut";

const press = (
  code: string,
  mods: Partial<Record<"ctrl" | "alt" | "shift" | "meta", boolean>> = {},
) => ({
  code,
  ctrl: mods.ctrl ?? false,
  alt: mods.alt ?? false,
  shift: mods.shift ?? false,
  meta: mods.meta ?? false,
});

describe("acceleratorFrom", () => {
  it("builds the combination the backend parses", () => {
    expect(acceleratorFrom(press("Space", { ctrl: true, alt: true }))).toEqual({
      ok: true,
      accelerator: "Ctrl+Alt+Space",
    });
    expect(acceleratorFrom(press("KeyK", { ctrl: true, shift: true }))).toEqual({
      ok: true,
      accelerator: "Ctrl+Shift+K",
    });
    expect(acceleratorFrom(press("PageDown", { alt: true }))).toEqual({
      ok: true,
      accelerator: "Alt+PageDown",
    });
    expect(acceleratorFrom(press("F8", { meta: true }))).toEqual({
      ok: true,
      accelerator: "Super+F8",
    });
    expect(acceleratorFrom(press("Digit1", { ctrl: true }))).toEqual({
      ok: true,
      accelerator: "Ctrl+1",
    });
  });

  it("refuses a key with no modifier: a global bare key takes that key everywhere", () => {
    expect(acceleratorFrom(press("KeyA"))).toEqual({ ok: false, reason: "modifier" });
    expect(acceleratorFrom(press("F8", { shift: true }))).toEqual({
      ok: false,
      reason: "modifier",
    });
  });

  it("refuses a key the backend does not know", () => {
    expect(acceleratorFrom(press("IntlYen", { ctrl: true }))).toEqual({
      ok: false,
      reason: "unsupported",
    });
    expect(acceleratorFrom(press("F13", { ctrl: true }))).toEqual({
      ok: false,
      reason: "unsupported",
    });
  });
});

describe("prettyAccelerator", () => {
  it("shows the Windows key as Win, and spaces the parts", () => {
    expect(prettyAccelerator("Super+Shift+K")).toBe("Win + Shift + K");
    expect(prettyAccelerator("Ctrl+Alt+Space")).toBe("Ctrl + Alt + Space");
    expect(prettyAccelerator("Control+KeyA")).toBe("Ctrl + KeyA");
    expect(prettyAccelerator("CmdOrCtrl+PageDown")).toBe("Ctrl + PageDown");
  });
});

describe("isModifierCode", () => {
  it("knows every modifier key, and only those", () => {
    for (const code of [
      "ControlLeft",
      "ControlRight",
      "AltLeft",
      "AltRight",
      "ShiftLeft",
      "ShiftRight",
      "MetaLeft",
      "MetaRight",
    ]) {
      expect(isModifierCode(code)).toBe(true);
    }
    expect(isModifierCode("KeyA")).toBe(false);
    expect(isModifierCode("Space")).toBe(false);
  });
});
