/**
 * Icon physics: gravity plus real AABB contacts with restitution, friction and
 * a sleep state.
 *
 * Pure on purpose (no DOM, no timers) so the collision behaviour can be
 * exercised outside the app. The inline version it replaces re-injected a
 * sideways shove (up to 40px and 260 px/s^2 per frame) into *every* frame an
 * icon overlapped a neighbour, so an icon resting on another one could never
 * drop below the "at rest" speed test: it slid, fell off, landed, bounced and
 * repeated — an endless bounce/jitter instead of coming to rest.
 *
 * Rules here:
 *  - one resolution per contact per step, along the axis of least penetration;
 *  - restitution applies only above IMPACT_MIN; a slower contact is a resting
 *    contact and simply cancels the velocity on that axis (no bounce, no
 *    re-injection of energy);
 *  - every bounce loses CONTACT_LOSS on top of the restitution factor, so the
 *    bounce train terminates in a bounded number of steps even at bounce=0.95;
 *  - an icon that stays slow while supported for SLEEP_TIME goes to sleep
 *    (the caller then stops its animation loop).
 */

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Solid extends Box {
  id: string;
}

export interface WorldBounds {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface BodyState {
  x: number;
  y: number;
  vx: number;
  vy: number;
  w: number;
  h: number;
  /** seconds this body has been slow *and* supported */
  restTime: number;
  /** id of the body it currently rests on (null: floor or mid-air) */
  restingOn: string | null;
  /** consecutive bounces: capped so an icon cannot pogo forever */
  bounces: number;
}

export interface StepInput {
  body: BodyState;
  gravity: number;
  /** settings.bounce: 0..0.95 */
  restitution: number;
  bounds: WorldBounds;
  others: Solid[];
  dt: number;
}

export interface StepOutput {
  x: number;
  y: number;
  vx: number;
  vy: number;
  restTime: number;
  restingOn: string | null;
  bounces: number;
  /** true once the body has been at rest long enough to stop animating */
  settled: boolean;
  /** number of contacts that produced a real bounce this step (for the squash cue) */
  impacts: number;
}

const AIR_DRAG = 0.12;
const FLOOR_GAP = 6;
/** below this, a contact is resting: no bounce, just stop on that axis */
const IMPACT_MIN = 90;
/** px/s removed per bounce so the bounce train always ends */
const CONTACT_LOSS = 25;
const SLEEP_SPEED = 16;
const SLEEP_TIME = 0.15;
/** per second, while standing on something */
const SURFACE_FRICTION = 6;
const WALL_RESTITUTION = 0.6;
const CEILING_RESTITUTION = 0.5;
const SIDE_FRICTION = 0.85;
const TIP_ACCEL = 110;
/** beyond this lean an icon tips off the pile instead of balancing on a corner */
const TIP_LEAN = 0.5;
/**
 * Sideways separation is spread over several frames instead of teleporting the
 * icon out of a deep overlap in one step.
 */
const MAX_PUSH = 14;
/**
 * Hard cap on consecutive bounces. Restitution alone only decays the bounce
 * train geometrically, so at bounce=0.95 an icon could keep pogoing for ~10s;
 * after this many contacts it is treated as resting instead.
 */
const MAX_BOUNCES = 6;

interface MotionState {
  x: number;
  y: number;
  vx: number;
  vy: number;
  w: number;
  h: number;
}

function resolveSolidCollisions(
  state: MotionState,
  others: Solid[],
  restitution: number,
  bounceFn: (speed: number, rest: number) => number,
): { x: number; y: number; vx: number; vy: number; supported: boolean; restingOn: string | null } {
  let { x, y, vx, vy, w, h } = state;
  let supported = false;
  let restingOn: string | null = null;

  for (const o of others) {
    const overlapX = Math.min(x + w, o.x + o.w) - Math.max(x, o.x);
    const overlapY = Math.min(y + h, o.y + o.h) - Math.max(y, o.y);
    if (overlapX <= 0 || overlapY <= 0) continue;

    if (overlapY <= overlapX && overlapY <= h / 2) {
      if (y + h / 2 <= o.y + o.h / 2) {
        // we are above: land on it
        y = o.y - h;
        supported = true;
        restingOn = o.id;
        vy = vy > 0 ? -bounceFn(vy, restitution) : 0;
      } else {
        // we are under it: bump our head
        y = o.y + o.h;
        vy = vy < 0 ? bounceFn(vy, restitution) : 0;
      }
    } else {
      // side contact: slide out of the overlap and lose horizontal speed
      const push = Math.min(overlapX + 0.5, MAX_PUSH);
      if (x + w / 2 <= o.x + o.w / 2) {
        x = Math.max(o.x - w, x - push);
        vx = -bounceFn(vx, restitution * 0.8);
      } else {
        x = Math.min(o.x + o.w, x + push);
        vx = bounceFn(vx, restitution * 0.8);
      }
      vy *= SIDE_FRICTION;
    }
  }

  return { x, y, vx, vy, supported, restingOn };
}

function resolveBoundaries(
  state: { x: number; y: number; vx: number; vy: number; w: number },
  bounds: WorldBounds,
  bounceFn: (speed: number, rest: number) => number,
): { x: number; y: number; vx: number; vy: number } {
  let { x: nx, y: ny, vx: nvx, vy: nvy, w } = state;

  if (nx < bounds.x) {
    nx = bounds.x;
    if (nvx < 0) nvx = bounceFn(nvx, WALL_RESTITUTION);
  }
  if (nx > bounds.x + bounds.w - w) {
    nx = bounds.x + bounds.w - w;
    if (nvx > 0) nvx = -bounceFn(nvx, WALL_RESTITUTION);
  }
  if (ny < bounds.y) {
    ny = bounds.y;
    if (nvy < 0) nvy = bounceFn(nvy, CEILING_RESTITUTION);
  }

  return { x: nx, y: ny, vx: nvx, vy: nvy };
}

function applySupportFriction(
  pos: { x: number; vx: number; w: number; restingOn: string | null },
  others: Solid[],
  dt: number,
): { vx: number; tipping: boolean } {
  const { x, restingOn, w } = pos;
  let nvx = pos.vx * (1 - Math.min(1, SURFACE_FRICTION * dt));
  let tipping = false;
  if (restingOn) {
    const o = others.find((it) => it.id === restingOn);
    if (o) {
      const lean = (x + w / 2 - (o.x + o.w / 2)) / ((w + o.w) / 2);
      if (Math.abs(lean) > TIP_LEAN) {
        nvx += Math.sign(lean) * TIP_ACCEL * dt;
        tipping = true;
      }
    }
  }
  return { vx: nvx, tipping };
}

export function stepBody(input: StepInput): StepOutput {
  const { body, gravity, restitution, bounds, others, dt } = input;
  const { w, h } = body;
  let { x, y, vx, vy } = body;
  let bounces = body.bounces ?? 0;
  let impacts = 0;

  // 1. integrate (semi-implicit Euler)
  vy += gravity * dt;
  vx *= 1 - AIR_DRAG * dt;
  x += vx * dt;
  y += vy * dt;

  const reflected = (speed: number, rest: number): number => {
    const a = Math.abs(speed);
    if (a <= IMPACT_MIN || bounces >= MAX_BOUNCES) return 0;
    const bouncedVal = a * rest - CONTACT_LOSS;
    return bouncedVal > IMPACT_MIN ? bouncedVal : 0;
  };
  const bounced = (speed: number, rest: number): number => {
    const v = reflected(speed, rest);
    if (v > 0) {
      bounces++;
      impacts++;
    }
    return v;
  };

  // 2. floor
  let supported = false;
  let restingOn: string | null = null;
  const floorY = bounds.y + bounds.h - h - FLOOR_GAP;
  if (y >= floorY) {
    y = floorY;
    supported = true;
    if (vy > 0) vy = -bounced(vy, restitution);
  }

  // 3. other icons
  const contact = resolveSolidCollisions({ x, y, vx, vy, w, h }, others, restitution, bounced);
  x = contact.x;
  y = contact.y;
  vx = contact.vx;
  vy = contact.vy;
  if (contact.supported) {
    supported = true;
    restingOn = contact.restingOn;
  }

  // 4. walls and ceiling
  const bounded = resolveBoundaries({ x, y, vx, vy, w }, bounds, bounced);
  x = bounded.x;
  y = bounded.y;
  vx = bounded.vx;
  vy = bounded.vy;

  // 5. friction on the support, and tipping off an unstable pile
  let tipping = false;
  if (supported) {
    const fric = applySupportFriction({ x, vx, w, restingOn }, others, dt);
    vx = fric.vx;
    tipping = fric.tipping;
  }

  // 6. sleep: slow *and* supported for long enough
  const slow = Math.abs(vx) < SLEEP_SPEED && Math.abs(vy) < SLEEP_SPEED;
  const atRest = supported && slow && !tipping;
  const restTime = atRest ? body.restTime + dt : 0;
  if (atRest && restTime > SLEEP_TIME) bounces = 0;

  return {
    x,
    y,
    vx,
    vy,
    restTime,
    restingOn: supported ? restingOn : null,
    bounces,
    settled: restTime >= SLEEP_TIME,
    impacts,
  };
}

/** True when nothing is under the body any more (its support was moved away). */
export function supportGone(body: Box, others: Solid[]): boolean {
  const bottom = body.y + body.h;
  for (const o of others) {
    const overlapX = Math.min(body.x + body.w, o.x + o.w) - Math.max(body.x, o.x);
    const gap = bottom - o.y;
    if (overlapX > 4 && gap >= -1 && gap < 12) return false;
  }
  return true;
}
