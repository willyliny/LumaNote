/**
 * Small runtime signals for the 2.5D sprite, shared by every instance in a window:
 *  - reduced motion (-> the sprite falls back to the flat art, per the designer's delivery note)
 *  - devicePixelRatio (-> @1x or @2x layers)
 *  - "awake": false while the document is hidden or no pointer was seen over the window for IDLE_PAUSE_MS.
 *    While asleep every sprite animation is paused (animation-play-state), so an idle desktop costs nothing.
 *    A pointer event over the window, a visible document or a sprite state change wakes it.
 * Event listeners are attached only while something is subscribed.
 */
import { useSyncExternalStore } from 'react';

export const IDLE_PAUSE_MS = 30_000;

/* ------------------------------ reduced motion ------------------------------ */
const RM_QUERY = '(prefers-reduced-motion: reduce)';
function subscribeRm(cb: () => void) {
  let mql: MediaQueryList | null = null;
  try {
    mql = window.matchMedia(RM_QUERY);
    mql.addEventListener('change', cb);
  } catch {
    /* no matchMedia: treated as "no preference" */
  }
  return () => mql?.removeEventListener('change', cb);
}
const getRm = () => {
  try {
    return window.matchMedia(RM_QUERY).matches;
  } catch {
    return false;
  }
};
export const useReducedMotion = (): boolean => useSyncExternalStore(subscribeRm, getRm, () => false);

/* ------------------------------ device pixel ratio ------------------------------ */
function subscribeDpr(cb: () => void) {
  let mql: MediaQueryList | null = null;
  const on = () => {
    mql?.removeEventListener('change', on);
    cb();
    arm();
  };
  const arm = () => {
    try {
      mql = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      mql.addEventListener('change', on);
    } catch {
      mql = null;
    }
  };
  arm();
  return () => mql?.removeEventListener('change', on);
}
export const useDevicePixelRatio = (): number =>
  useSyncExternalStore(subscribeDpr, () => window.devicePixelRatio || 1, () => 1);

/** 192 px layers are enough while the sprite needs no more device pixels than that; else the 384 px ones. */
export function pickScale(cssSize: number, dpr: number): 1 | 2 {
  return cssSize * dpr <= 192 + 0.5 ? 1 : 2;
}

/* ------------------------------ awake / asleep ------------------------------ */
let awake = true;
let lastActivity = 0;
let sleepTimer: number | undefined;
const subs = new Set<() => void>();
const now = () => performance.now();

function setAwake(v: boolean) {
  if (awake === v) return;
  awake = v;
  subs.forEach((f) => f());
}
function armSleep() {
  if (sleepTimer !== undefined) return;
  const check = () => {
    sleepTimer = undefined;
    const left = IDLE_PAUSE_MS - (now() - lastActivity);
    if (left > 0) sleepTimer = window.setTimeout(check, left + 5);
    else setAwake(false);
  };
  sleepTimer = window.setTimeout(check, IDLE_PAUSE_MS);
}
/** pointer seen / state changed / document shown: resume and restart the idle countdown */
export function wakeSprites() {
  lastActivity = now();
  if (document.hidden) return;
  setAwake(true);
  armSleep();
}
const onVisibility = () => {
  if (document.hidden) setAwake(false);
  else wakeSprites();
};
const onPointer = () => {
  // cheap on the hot path: one timestamp; only a sleeping sprite does more
  lastActivity = now();
  if (!awake) wakeSprites();
};
function attach() {
  lastActivity = now();
  awake = !document.hidden;
  if (awake) armSleep();
  window.addEventListener('pointermove', onPointer, { passive: true, capture: true });
  window.addEventListener('pointerdown', onPointer, { passive: true, capture: true });
  document.addEventListener('pointerover', onPointer, { passive: true, capture: true });
  document.addEventListener('visibilitychange', onVisibility);
}
function detach() {
  window.removeEventListener('pointermove', onPointer, { capture: true });
  window.removeEventListener('pointerdown', onPointer, { capture: true });
  document.removeEventListener('pointerover', onPointer, { capture: true });
  document.removeEventListener('visibilitychange', onVisibility);
  window.clearTimeout(sleepTimer);
  sleepTimer = undefined;
}
function subscribeAwake(cb: () => void) {
  if (subs.size === 0) attach();
  subs.add(cb);
  return () => {
    subs.delete(cb);
    if (subs.size === 0) detach();
  };
}
export const useSpritesAwake = (): boolean => useSyncExternalStore(subscribeAwake, () => awake, () => true);
