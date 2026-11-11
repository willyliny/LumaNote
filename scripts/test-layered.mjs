#!/usr/bin/env node
/**
 * Negative + positive tests for the layered-art validation and the sync script, run in a temp dir.
 *   node scripts/test-layered.mjs        (npm run test:layered)
 * Starts from a copy of docs/design/skins/night-letter, breaks one thing per case and expects a rejection.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { validateLayeredDir } from './layered-validate.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SRC_SKIN = path.join(ROOT, 'docs/design/skins/night-letter');
const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'luma-layered-test-'));
process.on('exit', () => fs.rmSync(tmpRoot, { recursive: true, force: true }));

function crcChunk(type, data) {
  const t = Buffer.from(type, 'latin1');
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(zlib.crc32(Buffer.concat([t, data])));
  return Buffer.concat([len, t, data, crc]);
}
/** tiny solid PNG; colour 6 = RGBA, 2 = RGB */
function makePng(size, colour = 6) {
  const ch = colour === 6 ? 4 : 3;
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = colour;
  const row = Buffer.alloc(1 + size * ch);
  const raw = Buffer.concat(Array.from({ length: size }, () => row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    crcChunk('IHDR', ihdr),
    crcChunk('IDAT', zlib.deflateSync(raw)),
    crcChunk('IEND', Buffer.alloc(0)),
  ]);
}

let n = 0;
/** fresh copy of the real skin folder (layered/ only matters) */
function fresh() {
  const dir = path.join(tmpRoot, `case-${n++}`, 'night-letter');
  fs.cpSync(path.join(SRC_SKIN, 'layered'), path.join(dir, 'layered'), { recursive: true });
  return dir;
}
const jsonEdit = (dir, file, fn) => {
  const p = path.join(dir, 'layered', file);
  const j = JSON.parse(fs.readFileSync(p, 'utf8'));
  fn(j);
  fs.writeFileSync(p, JSON.stringify(j, null, 2));
};

const cases = [];
const reject = (name, mutate, expect) => cases.push({ name, mutate, expect });

reject('missing @2x file', (d) => fs.rmSync(path.join(d, 'layered/body@2x.png')), /body@2x\.png.*missing/);
reject('missing @1x of a state overlay', (d) => fs.rmSync(path.join(d, 'layered/state-approval@1x.png')), /state-approval@1x\.png.*missing/);
reject('wrong dimensions (100x100 instead of 192)', (d) => fs.writeFileSync(path.join(d, 'layered/pen@1x.png'), makePng(100)), /pen@1x\.png: is 100x100, expected 192x192/);
reject('@2x is 192 not 384', (d) => fs.writeFileSync(path.join(d, 'layered/pen@2x.png'), makePng(192)), /pen@2x\.png: is 192x192, expected 384x384/);
reject('colour type without alpha', (d) => fs.writeFileSync(path.join(d, 'layered/glow@1x.png'), makePng(192, 2)), /glow@1x\.png: colour type 2 has no alpha/);
reject('not a PNG', (d) => fs.writeFileSync(path.join(d, 'layered/glow@1x.png'), 'GIF89a nope, not a png at all, still long enough to pass a length check'), /not a PNG/);
reject('corrupted PNG chunk (CRC)', (d) => {
  const f = path.join(d, 'layered/shadow@1x.png');
  const b = fs.readFileSync(f);
  b[40] ^= 0xff;
  fs.writeFileSync(f, b);
}, /shadow@1x\.png: bad CRC/);
reject('9 base layers', (d) => jsonEdit(d, 'layers.json', (j) => j.layers.push({ id: 'extra', file: 'pen', role: 'accessory' })), /9 layers/);
reject('state mounts 9 layers (overlay on top of 8 visible)', (d) => jsonEdit(d, 'states.json', (j) => (j.states.listening.hiddenBaseLayers = [])), /states\.listening mounts 9 layers/);
reject('file value with ../', (d) => jsonEdit(d, 'layers.json', (j) => (j.layers[3].file = '../body')), /not a safe file name/);
reject('file value with a dot or slash', (d) => jsonEdit(d, 'layers.json', (j) => (j.layers[3].file = 'sub/body.x')), /not a safe file name/);
reject('overlay file escaping the folder', (d) => jsonEdit(d, 'states.json', (j) => (j.states.paused.overlay.file = '..\\..\\x')), /overlay must be null or/);
reject('layer id with spaces/uppercase', (d) => jsonEdit(d, 'layers.json', (j) => (j.layers[0].id = 'Shadow Layer')), /not a safe id/);
reject('duplicate layer id', (d) => jsonEdit(d, 'layers.json', (j) => (j.layers[1].id = 'shadow')), /duplicated/);
reject('unknown role', (d) => jsonEdit(d, 'layers.json', (j) => (j.layers[1].role = 'sparkle')), /role "sparkle"/);
reject('states.json id mismatch', (d) => jsonEdit(d, 'states.json', (j) => (j.id = 'other')), /does not match folder/);
reject('unknown hidden layer id', (d) => jsonEdit(d, 'states.json', (j) => (j.states.idle.hiddenBaseLayers = ['nope'])), /hiddenBaseLayers/);
reject('missing state', (d) => jsonEdit(d, 'states.json', (j) => delete j.states.paused), /exactly/);
reject('blink while eyes closed', (d) => jsonEdit(d, 'states.json', (j) => (j.states.paused.blink = true)), /blink must be false/);
reject('layers.json is not JSON', (d) => fs.writeFileSync(path.join(d, 'layered/layers.json'), '{nope'), /layers\.json does not parse/);
reject('missing states.json', (d) => fs.rmSync(path.join(d, 'layered/states.json')), /missing states\.json/);
reject('over the byte budget', (d) => {
  // a valid but fat PNG: incompressible noise in the IDAT of body@2x
  const size = 384;
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const raw = Buffer.alloc(size * (1 + size * 4));
  for (let i = 0; i < raw.length; i++) raw[i] = (i * 2654435761) >>> 24;
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    crcChunk('IHDR', ihdr),
    crcChunk('IDAT', zlib.deflateSync(raw, { level: 0 })),
    crcChunk('IEND', Buffer.alloc(0)),
  ]);
  fs.writeFileSync(path.join(d, 'layered/body-back@2x.png'), png);
}, /not under 600000/);

let failed = 0;
const ok = validateLayeredDir(fresh(), 'night-letter');
try {
  assert.equal(ok.ok, true, `the untouched delivery must pass: ${ok.errors.join('; ')}`);
  console.log(`ok   untouched night-letter passes (${ok.info.pngs} png, ${ok.bytes} bytes)`);
} catch (e) {
  failed++;
  console.log(`FAIL ${e.message}`);
}
for (const c of cases) {
  const dir = fresh();
  c.mutate(dir);
  const r = validateLayeredDir(dir, 'night-letter');
  const text = r.errors.join(' | ');
  if (!r.ok && c.expect.test(text) && r.files.length === 0) console.log(`ok   rejects: ${c.name}`);
  else {
    failed++;
    console.log(`FAIL ${c.name}: ok=${r.ok} errors=${text || '(none)'}`);
  }
}

/* ---- end to end through the sync script: flat art must still be copied, layered must be absent ---- */
{
  const src = path.join(tmpRoot, 'e2e-src');
  const dest = path.join(tmpRoot, 'e2e-dest');
  fs.cpSync(SRC_SKIN, path.join(src, 'night-letter'), { recursive: true });
  fs.writeFileSync(path.join(src, 'night-letter/layered/glow@1x.png'), makePng(100));
  const run = (extra = []) =>
    spawnSync(process.execPath, [path.join(ROOT, 'scripts/sync-skins.mjs'), '--src', src, '--dest', dest, '--tokens-css', path.join(tmpRoot, 't.css'), '--family-json', path.join(tmpRoot, 'f.json'), '--registry', path.join(tmpRoot, 'none.ts'), ...extra], { encoding: 'utf8' });
  // 1. good layered first, so a stale copy exists
  fs.cpSync(path.join(SRC_SKIN, 'layered/glow@1x.png'), path.join(tmpRoot, 'glow-good.png'));
  const good = path.join(tmpRoot, 'glow-good.png');
  fs.copyFileSync(path.join(src, 'night-letter/layered/glow@1x.png'), path.join(tmpRoot, 'glow-bad.png'));
  fs.copyFileSync(good, path.join(src, 'night-letter/layered/glow@1x.png'));
  const r1 = run();
  const had = fs.existsSync(path.join(dest, 'night-letter/layered/layers.json'));
  // 2. now break it
  fs.copyFileSync(path.join(tmpRoot, 'glow-bad.png'), path.join(src, 'night-letter/layered/glow@1x.png'));
  const r2 = run();
  const flat = fs.existsSync(path.join(dest, 'night-letter/idle.svg')) && fs.existsSync(path.join(dest, 'night-letter/skin.json'));
  const stale = fs.existsSync(path.join(dest, 'night-letter/layered'));
  const checks = [
    ['sync of a good set copies layered/', had && r1.status === 0],
    ['sync of a broken set reports LAYERED-REJECT and exits 1', /LAYERED-REJECT night-letter/.test(r2.stdout) && r2.status === 1],
    ['broken set: flat SVG art is still synced', flat],
    ['broken set: no layered/ left behind (nothing half-copied, stale copy removed)', !stale],
  ];
  for (const [name, pass] of checks) {
    if (pass) console.log(`ok   e2e: ${name}`);
    else {
      failed++;
      console.log(`FAIL e2e: ${name}\n${r1.stdout}\n${r2.stdout}`);
    }
  }
}

console.log(failed ? `\n${failed} FAILED` : `\nall ${cases.length + 5} checks passed`);
process.exit(failed ? 1 : 0);
