/**
 * Every global setting the window can edit, and the row for it.
 *
 * One entry per setting: what it is called, its range, and the key it writes.
 * A row is built fresh each time it is asked for, so the same setting may be
 * shown in two places (a plugin's card and the Motion tab) without one of them
 * stealing the other's DOM node — which is how a row used to vanish from a card
 * the moment that card re-rendered.
 */

import type { FloatSettings } from "../widgets/lib";
import { el } from "./dom";
import { settings, updateSettings } from "./store";

/** Settings whose value is a number (`disabled` is a list, so it is neither). */
type NumericKey = {
  [K in keyof FloatSettings]: FloatSettings[K] extends number ? K : never;
}[keyof FloatSettings];

/** Settings whose value is a choice between fixed words. */
type ChoiceKey = {
  [K in keyof FloatSettings]: FloatSettings[K] extends string ? K : never;
}[keyof FloatSettings];

export interface NumberParam {
  key: NumericKey;
  label: string;
  min: number;
  max: number;
  step: number;
  unit?: string;
}

export interface ChoiceParam {
  key: ChoiceKey;
  label: string;
  options: string[];
}

export const NUMBER_PARAMS: NumberParam[] = [
  { key: "pet_speed", label: "pet speed", min: 0, max: 2, step: 0.1 },
  { key: "gravity", label: "gravity", min: 0, max: 5000, step: 50 },
  { key: "bounce", label: "bounce", min: 0, max: 0.9, step: 0.05 },
  { key: "floatiness", label: "float", min: 0, max: 2, step: 0.1 },
  { key: "float_amplitude", label: "float height", min: 0, max: 12, step: 0.5, unit: "px" },
  { key: "float_period", label: "float cycle", min: 2, max: 30, step: 1, unit: "s" },
  { key: "float_spread", label: "wave spread", min: 0, max: 200, step: 10, unit: "%" },
  { key: "animated_ratio", label: "animated icons", min: 0, max: 100, step: 10, unit: "%" },
  { key: "viz_gain", label: "viz gain", min: 0.2, max: 3, step: 0.1, unit: "x" },
  { key: "viz_fps", label: "viz fps", min: 5, max: 60, step: 5, unit: "fps" },
  { key: "sysmon_interval", label: "sysmon refresh", min: 250, max: 5000, step: 250, unit: "ms" },
];

export const CHOICE_PARAMS: ChoiceParam[] = [
  { key: "single_click", label: "single click", options: ["drop", "hop", "nothing"] },
  { key: "double_click", label: "double click", options: ["launch", "drop", "nothing"] },
  { key: "animation_mode", label: "animation mode", options: ["wave", "sync", "gentle", "static"] },
];

export function numberParam(key: string): NumberParam | undefined {
  return NUMBER_PARAMS.find((p) => p.key === key);
}

export function choiceParam(key: string): ChoiceParam | undefined {
  return CHOICE_PARAMS.find((p) => p.key === key);
}

function label(text: string): HTMLSpanElement {
  return el("span", "slider-label", text);
}

/** A slider row, live: dragging it writes the setting (debounced). */
export function numberRow(key: string, def = numberParam(key)): HTMLElement | undefined {
  if (!def) return undefined;
  const value = settings()[def.key];
  const row = el("div", "slider-row");
  const input = el("input", "");
  input.type = "range";
  input.min = String(def.min);
  input.max = String(def.max);
  input.step = String(def.step);
  input.value = String(value);
  const shown = el("span", "slider-val", def.unit ? `${value}${def.unit}` : String(value));
  input.addEventListener("input", () => {
    const next = Number(input.value);
    shown.textContent = def.unit ? `${next}${def.unit}` : String(next);
    void updateSettings({ [def.key]: next } as Partial<FloatSettings>);
  });
  row.append(label(def.label), input, shown);
  return row;
}

/** A dropdown row, live: choosing writes the setting straight away. */
export function choiceRow(key: string, def = choiceParam(key)): HTMLElement | undefined {
  if (!def) return undefined;
  const row = el("div", "slider-row");
  const select = el("select", "select");
  for (const option of def.options) {
    const opt = document.createElement("option");
    opt.value = option;
    opt.textContent = option;
    select.append(opt);
  }
  select.value = settings()[def.key];
  select.addEventListener("change", () =>
    void updateSettings({ [def.key]: select.value }, { now: true }),
  );
  row.append(label(def.label), select);
  return row;
}

/**
 * A row for a shared setting, by name — what a plugin asks for through
 * `PluginSettingsContext.getSharedRow`. Unknown names return `undefined`, so a
 * plugin asking for a setting this build does not have draws nothing rather
 * than an empty row.
 */
export function sharedRow(name: string): HTMLElement | undefined {
  return numberRow(name) ?? choiceRow(name);
}
