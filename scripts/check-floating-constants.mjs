#!/usr/bin/env node
/**
 * Build-time check that the floating window numbers agree everywhere and that the wheel fits.
 *   node scripts/check-floating-constants.mjs     (npm run check:floating, part of npm run build)
 * Sources: src/features/floating/layout.ts (owner), src-tauri/src/geometry.rs, src-tauri/tauri.conf.json,
 * src/styles/tokens.css (--size-sprite) and floating.css fallbacks. The same equalities are asserted by
 * `cargo test -p lumanote-shell` (geometry::tests::constants_match_frontend).
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (p) => fs.readFileSync(path.join(ROOT, p), 'utf8');
const errors = [];
const ts = read('src/features/floating/layout.ts');
const tsNum = (n) => {
  const m = new RegExp(`^export const ${n} = (\\d+(?:\\.\\d+)?);`, 'm').exec(ts);
  if (!m) errors.push(`layout.ts: ${n} not found as "export const ${n} = <number>;"`);
  return m ? Number(m[1]) : NaN;
};
const rs = read('src-tauri/src/geometry.rs');
const rsNum = (n) => {
  const m = new RegExp(`pub const ${n}: f64 = (\\d+(?:\\.\\d+)?);`).exec(rs);
  if (!m) errors.push(`geometry.rs: ${n} not found`);
  return m ? Number(m[1]) : NaN;
};
const expect = (what, a, b) => a === b || errors.push(`${what}: ${a} != ${b}`);

const sprite = tsNum('FLOATING_SPRITE_PX');
const collapsed = tsNum('FLOATING_COLLAPSED_PX');
const expanded = tsNum('FLOATING_EXPANDED_PX');
const iw = tsNum('WHEEL_ITEM_W');
const ih = tsNum('WHEEL_ITEM_H');
const rx = tsNum('WHEEL_RX');
const ry = tsNum('WHEEL_RY');
const gapMin = tsNum('WHEEL_MIN_GAP');
const target = tsNum('MIN_TARGET_PX');

expect('collapsed: layout.ts vs geometry.rs', collapsed, rsNum('COLLAPSED_LOGICAL'));
expect('expanded: layout.ts vs geometry.rs', expanded, rsNum('EXPANDED_LOGICAL'));
const conf = JSON.parse(read('src-tauri/tauri.conf.json'));
const win = conf.app.windows.find((w) => w.label === 'floating');
expect('collapsed: layout.ts vs tauri.conf.json width', collapsed, win?.width);
expect('collapsed: layout.ts vs tauri.conf.json height', collapsed, win?.height);
const tok = /--size-sprite:\s*(\d+)px/.exec(read('src/styles/tokens.css'));
expect('sprite: layout.ts vs tokens.css --size-sprite', sprite, tok ? Number(tok[1]) : undefined);
const css = read('src/features/floating/floating.css');
const cw = /var\(--wheel-item-w,\s*(\d+)px\)/.exec(css);
const ch = /var\(--wheel-item-h,\s*(\d+)px\)/.exec(css);
expect('wheel item width: layout.ts vs floating.css fallback', iw, cw ? Number(cw[1]) : undefined);
expect('wheel item height: layout.ts vs floating.css fallback', ih, ch ? Number(ch[1]) : undefined);

/* ---- geometry: same maths as layout.ts wheelSlot/boxGap ---- */
const slots = Array.from({ length: 6 }, (_, i) => {
  const a = ((-90 + 60 * i) * Math.PI) / 180;
  return { cx: Math.round(rx * Math.cos(a)), cy: Math.round(ry * Math.sin(a)), w: iw, h: ih };
});
const gap = (a, b) => Math.max(Math.abs(a.cx - b.cx) - (a.w + b.w) / 2, Math.abs(a.cy - b.cy) - (a.h + b.h) / 2);
const sbox = { cx: 0, cy: 0, w: sprite, h: sprite };
let minItems = Infinity;
let minSprite = Infinity;
for (let i = 0; i < 6; i++) {
  minSprite = Math.min(minSprite, gap(slots[i], sbox));
  for (let j = i + 1; j < 6; j++) minItems = Math.min(minItems, gap(slots[i], slots[j]));
}
if (minItems < gapMin) errors.push(`wheel items are only ${minItems}px apart (need ${gapMin})`);
if (minSprite < gapMin) errors.push(`a wheel item is only ${minSprite}px from the sprite box (need ${gapMin})`);
if (iw < target || ih < target) errors.push(`wheel items ${iw}x${ih} are below the ${target}px target`);
const reach = Math.max(...slots.map((s) => Math.max(Math.abs(s.cx) + iw / 2, Math.abs(s.cy) + ih / 2)));
if (reach + 16 > expanded / 2) errors.push(`wheel reaches ${reach}px from the centre; the ${expanded}px window leaves under 16px for shadows`);
if (collapsed - sprite < 16) errors.push('collapsed window leaves under 8px per side around the sprite');
for (const s of [1, 1.25, 1.5, 2]) {
  const d = ((expanded - collapsed) * s) / 2;
  if (!Number.isInteger(d)) errors.push(`at ${s * 100}% scaling the expanded window cannot be centred on whole physical pixels (${d})`);
}

if (errors.length) {
  console.error('floating constants check FAILED\n - ' + errors.join('\n - '));
  process.exit(1);
}
console.log(
  `floating constants ok: sprite ${sprite}, collapsed ${collapsed}, expanded ${expanded}; wheel gap items>=${minItems}px sprite>=${minSprite}px, reach ${reach}px`,
);
