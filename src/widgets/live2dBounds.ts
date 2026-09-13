/**
 * Boundary math for the Live2D widget.
 *
 * The model is always fitted to the widget box, and the box is what the wheel
 * zoom changes. That is what keeps the model inside the boundary: scaling the
 * sprite past its canvas instead just cropped the character at the widget edge,
 * and a box bigger than the desktop put half the model off-screen.
 *
 * Kept free of DOM/PIXI so the numbers can be checked outside the app.
 */

export const BASE_W = 300;
export const BASE_H = 400;
/** zoom range, as a multiple of the base box */
export const MIN_SCALE = 0.5;
export const MAX_SCALE = 3;
/** desktop margin kept visible around the widget */
export const EDGE = 8;

export interface Area {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Point {
  x: number;
  y: number;
}

export interface Box {
  w: number;
  h: number;
}

/** Largest zoom whose box still fits inside the desktop. */
export function maxScaleFor(area: Area): number {
  return Math.max(
    MIN_SCALE,
    Math.min(MAX_SCALE, (area.w - EDGE * 2) / BASE_W, (area.h - EDGE * 2) / BASE_H),
  );
}

/** The clamped zoom and the box it produces. */
export function boxFor(requested: number, area: Area): { scale: number; w: number; h: number } {
  const scale = Math.max(MIN_SCALE, Math.min(maxScaleFor(area), requested));
  return { scale, w: Math.round(BASE_W * scale), h: Math.round(BASE_H * scale) };
}

/** The zoom a stored box implies (used when the record has w/h but no scale). */
export function scaleForBox(w: number, h: number): number {
  const avg = (w / BASE_W + h / BASE_H) / 2;
  return Math.max(MIN_SCALE, Math.min(MAX_SCALE, avg));
}

/** Position of the box, nudged so the whole box stays inside the desktop. */
export function fitInside(pos: Point, box: Box, area: Area): Point {
  return {
    x: Math.min(Math.max(pos.x, area.x), Math.max(area.x, area.x + area.w - box.w)),
    y: Math.min(Math.max(pos.y, area.y), Math.max(area.y, area.y + area.h - box.h)),
  };
}
