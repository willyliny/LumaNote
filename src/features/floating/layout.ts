/**
 * Floating window geometry, defined ONCE on the frontend side. The Rust shell has the other copy:
 *
 *   src-tauri/src/geometry.rs      COLLAPSED_LOGICAL / EXPANDED_LOGICAL
 *   src-tauri/tauri.conf.json      windows[floating].width / height (= collapsed)
 *
 * They cannot drift silently: `cargo test -p lumanote-shell` (geometry::tests::constants_match_frontend) and
 * `npm run check:floating` (part of `npm run build`) both compare the numbers below with those files and
 * check the wheel geometry. Keep each constant on ONE line, `export const NAME = <number>;`.
 *
 * Why these numbers:
 *  - sprite 96: the 2.5D art is designed for ~96 CSS px.
 *  - collapsed 128: 16 px of transparent room around the sprite for tilt and the float, still small enough
 *    to sit on the desktop unnoticed (the window itself swallows clicks, so it is not made larger).
 *  - expanded 344: wheel items (84 x 56) sit on an ellipse RX x RY around the sprite. With RX 120 / RY 92:
 *    the gap to the sprite box is >= 14 px sideways and 16 px above/below, neighbouring items are >= 8 px
 *    apart, and the outermost item edge is 146 px from the centre, leaving 26 px of the 172 px half-window
 *    for the items' shadow. 344 - 128 = 216 is a multiple of 8, so at 100/125/150/200 % scaling
 *    (344 - 128) * scale / 2 is a whole number of physical pixels: the window grows around the sprite centre
 *    with no half-pixel shift (340 would shift the sprite by half a pixel at 125 %).
 */
export const FLOATING_SPRITE_PX = 96;
export const FLOATING_COLLAPSED_PX = 128;
export const FLOATING_EXPANDED_PX = 344;

/** wheel item card (CSS: .wheel__item) */
export const WHEEL_ITEM_W = 84;
export const WHEEL_ITEM_H = 56;
/** slot ellipse radii, px from the sprite centre */
export const WHEEL_RX = 120;
export const WHEEL_RY = 92;
/** required clear space between any two wheel items, and between an item and the sprite box */
export const WHEEL_MIN_GAP = 8;
/** minimum pointer target */
export const MIN_TARGET_PX = 44;

export interface Box {
  cx: number;
  cy: number;
  w: number;
  h: number;
}

/** centre of slot i of n, clockwise from the top (same maths as Wheel.tsx) */
export function wheelSlot(i: number, n: number): { x: number; y: number } {
  const a = ((-90 + (360 / n) * i) * Math.PI) / 180;
  return { x: Math.round(WHEEL_RX * Math.cos(a)), y: Math.round(WHEEL_RY * Math.sin(a)) };
}

/** axis-aligned clear distance between two boxes (negative or 0 when they touch / overlap) */
export function boxGap(a: Box, b: Box): number {
  const gx = Math.abs(a.cx - b.cx) - (a.w + b.w) / 2;
  const gy = Math.abs(a.cy - b.cy) - (a.h + b.h) / 2;
  return Math.max(gx, gy);
}
