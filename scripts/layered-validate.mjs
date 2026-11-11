/**
 * Validation of a skin's 2.5D layered set (docs/briefs/astra-2p5d-night-letter-brief.zh-TW.md,
 * docs/design/skins/LAYERED-*-DELIVERY.zh-TW.md). Pure: reads from <dir>/layered, returns errors and the
 * exact list of files that may be shipped. Used by sync-skins.mjs and tested by test-layered.mjs.
 *
 * The frontend re-checks the same shape at runtime (src/skins/layered/schema.ts). Keep both in step.
 */
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

export const LAYERED_CANVAS = 384;
export const SCALES = { '1x': 192, '2x': 384 };
export const MAX_MOUNTED = 8;
/** all shipped bytes of one skin (png @1x+@2x of every referenced layer + both json files) must stay below this */
export const MAX_SKIN_BYTES = 600_000;
export const SAFE_ID = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
export const BASE_ROLES = ['shadow', 'glow', 'body', 'highlight', 'eyes-open', 'eyes-closed', 'accessory'];
export const STATE_NAMES = ['idle', 'listening', 'processing', 'approval', 'suggestion', 'paused'];
const MAX_JSON_BYTES = 64 * 1024;
const PNG_SIG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

const isObj = (v) => !!v && typeof v === 'object' && !Array.isArray(v);
const isSafeId = (v) => typeof v === 'string' && v.length <= 48 && SAFE_ID.test(v);

function readJson(file, errors, label) {
  try {
    const st = fs.lstatSync(file);
    if (!st.isFile()) return errors.push(`${label} is not a regular file`), null;
    if (st.size > MAX_JSON_BYTES) return errors.push(`${label} is larger than ${MAX_JSON_BYTES} bytes`), null;
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (e) {
    errors.push(fs.existsSync(file) ? `${label} does not parse: ${e.message}` : `missing ${label}`);
    return null;
  }
}

/** Checks a PNG buffer: signature, chunk walk with CRCs, IHDR size, alpha. Returns error strings. */
export function checkPng(buf, expected) {
  const errs = [];
  if (buf.length < 33 || !buf.subarray(0, 8).equals(PNG_SIG)) return ['not a PNG (bad signature)'];
  let off = 8;
  let first = true;
  let sawIdat = false;
  let sawIend = false;
  let colour = -1;
  let trns = false;
  while (off + 12 <= buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.toString('latin1', off + 4, off + 8);
    if (off + 12 + len > buf.length) {
      errs.push(`chunk ${type} runs past the end of the file`);
      break;
    }
    const body = buf.subarray(off + 8, off + 8 + len);
    const crc = buf.readUInt32BE(off + 8 + len);
    if (zlib.crc32(buf.subarray(off + 4, off + 8 + len)) !== crc) {
      errs.push(`bad CRC in chunk ${type}`);
      break;
    }
    if (first) {
      first = false;
      if (type !== 'IHDR' || len !== 13) {
        errs.push('first chunk is not a valid IHDR');
        break;
      }
      const w = body.readUInt32BE(0);
      const h = body.readUInt32BE(4);
      colour = body[9];
      if (w !== expected || h !== expected) errs.push(`is ${w}x${h}, expected ${expected}x${expected}`);
      if (body[8] !== 8 && body[8] !== 16) errs.push(`bit depth ${body[8]} unsupported`);
    }
    if (type === 'tRNS') trns = true;
    if (type === 'IDAT') sawIdat = true;
    if (type === 'IEND') {
      sawIend = true;
      break;
    }
    off += 12 + len;
  }
  if (!errs.length) {
    if (!sawIdat) errs.push('no IDAT chunk');
    if (!sawIend) errs.push('no IEND chunk');
    const alpha = colour === 4 || colour === 6 || (colour === 3 && trns);
    if (!alpha) errs.push(`colour type ${colour} has no alpha channel`);
  }
  return errs;
}

/**
 * @returns {{ ok: boolean, errors: string[], files: string[], bytes: number, info?: object }}
 *   files = names inside layered/ that may be copied (only when ok).
 */
export function validateLayeredDir(skinDir, skinId) {
  const errors = [];
  const dir = path.join(skinDir, 'layered');
  if (!fs.existsSync(dir)) return { ok: false, absent: true, errors: ['no layered/ folder'], files: [], bytes: 0 };
  if (!fs.lstatSync(dir).isDirectory()) return { ok: false, errors: ['layered is not a real folder'], files: [], bytes: 0 };

  const base = readJson(path.join(dir, 'layers.json'), errors, 'layers.json');
  const manifest = readJson(path.join(dir, 'states.json'), errors, 'states.json');
  if (!base || !manifest) return { ok: false, errors, files: [], bytes: 0 };

  /* ---- layers.json ---- */
  const baseIds = new Set();
  const files = new Map(); // base name (no scale, no ext) -> where it is used
  if (!isObj(base)) errors.push('layers.json must be an object');
  else {
    if (base.id !== skinId) errors.push(`layers.json id "${base.id}" does not match folder "${skinId}"`);
    if (base.canvas !== LAYERED_CANVAS) errors.push(`layers.json canvas must be ${LAYERED_CANVAS}`);
    const p = base.pivot;
    if (!isObj(p) || !(p.x >= 0 && p.x <= 1) || !(p.y >= 0 && p.y <= 1)) errors.push('layers.json pivot must be {x,y} within 0..1');
    if (!Array.isArray(base.layers) || base.layers.length === 0) errors.push('layers.json layers must be a non-empty array');
    else {
      if (base.layers.length > MAX_MOUNTED) errors.push(`layers.json has ${base.layers.length} layers (max ${MAX_MOUNTED})`);
      base.layers.forEach((l, i) => {
        if (!isObj(l)) return errors.push(`layers[${i}] is not an object`);
        if (!isSafeId(l.id)) errors.push(`layers[${i}].id ${JSON.stringify(l.id)} is not a safe id (a-z, 0-9, single hyphens)`);
        else if (baseIds.has(l.id)) errors.push(`layers[${i}].id "${l.id}" is duplicated`);
        else baseIds.add(l.id);
        if (!isSafeId(l.file)) errors.push(`layers[${i}].file ${JSON.stringify(l.file)} is not a safe file name (a-z, 0-9, single hyphens; no dots or slashes)`);
        else files.set(l.file, `layer ${l.id}`);
        if (!BASE_ROLES.includes(l.role)) errors.push(`layers[${i}].role ${JSON.stringify(l.role)} is not one of ${BASE_ROLES.join(', ')}`);
      });
      const roles = base.layers.map((l) => l?.role);
      if (!roles.includes('body')) errors.push('layers.json has no layer with role "body"');
      if (roles.includes('eyes-open') !== roles.includes('eyes-closed')) errors.push('eyes-open and eyes-closed must both exist or both be absent');
      for (const r of ['eyes-open', 'eyes-closed']) if (roles.filter((x) => x === r).length > 1) errors.push(`more than one ${r} layer`);
    }
  }

  /* ---- states.json ---- */
  if (!isObj(manifest)) errors.push('states.json must be an object');
  else {
    if (manifest.schemaVersion !== 1) errors.push('states.json schemaVersion must be 1');
    if (manifest.id !== skinId) errors.push(`states.json id "${manifest.id}" does not match folder "${skinId}"`);
    if (manifest.canvas !== LAYERED_CANVAS) errors.push(`states.json canvas must be ${LAYERED_CANVAS}`);
    if (manifest.base !== 'layers.json') errors.push('states.json base must be "layers.json"');
    if (!Number.isInteger(manifest.maxMountedLayers) || manifest.maxMountedLayers < 1 || manifest.maxMountedLayers > MAX_MOUNTED) {
      errors.push(`states.json maxMountedLayers must be an integer 1..${MAX_MOUNTED}`);
    }
    if (!isObj(manifest.states)) errors.push('states.json states must be an object');
    else {
      const names = Object.keys(manifest.states).sort().join(',');
      if (names !== [...STATE_NAMES].sort().join(',')) errors.push(`states.json must define exactly: ${STATE_NAMES.join(', ')}`);
      if (!STATE_NAMES.includes(manifest.defaultState) || !(manifest.defaultState in manifest.states)) {
        errors.push('states.json defaultState must be one of the six states');
      }
      for (const [name, s] of Object.entries(manifest.states)) {
        const w = `states.${name}`;
        if (!isObj(s)) {
          errors.push(`${w} is not an object`);
          continue;
        }
        if (typeof s.label !== 'string' || !s.label || s.label.length > 32) errors.push(`${w}.label must be a short string`);
        if (!Array.isArray(s.hiddenBaseLayers) || s.hiddenBaseLayers.some((id) => !baseIds.has(id))) {
          errors.push(`${w}.hiddenBaseLayers must list ids that exist in layers.json`);
        }
        if (s.eyes !== 'open' && s.eyes !== 'closed') errors.push(`${w}.eyes must be "open" or "closed"`);
        if (typeof s.blink !== 'boolean') errors.push(`${w}.blink must be boolean`);
        if (s.eyes === 'closed' && s.blink === true) errors.push(`${w}: blink must be false while the eyes are closed`);
        if (s.motion !== 'idle' && s.motion !== 'none') errors.push(`${w}.motion must be "idle" or "none"`);
        if (typeof s.composite !== 'string' || !/^states\/[a-z0-9-]+$/.test(s.composite)) errors.push(`${w}.composite must look like "states/<name>"`);
        let overlay = 0;
        if (s.overlay !== null) {
          const o = s.overlay;
          if (!isObj(o) || !isSafeId(o.id) || !isSafeId(o.file) || o.role !== 'state') {
            errors.push(`${w}.overlay must be null or {id, file, role:"state"} with safe ids`);
          } else {
            overlay = 1;
            if (baseIds.has(o.id)) errors.push(`${w}.overlay.id "${o.id}" collides with a base layer id`);
            files.set(o.file, `overlay of ${name}`);
          }
        }
        if (Array.isArray(s.hiddenBaseLayers) && Array.isArray(base?.layers)) {
          const mounted = base.layers.length - new Set(s.hiddenBaseLayers).size + overlay;
          if (mounted > MAX_MOUNTED) errors.push(`${w} mounts ${mounted} layers (max ${MAX_MOUNTED})`);
          if (Number.isInteger(manifest.maxMountedLayers) && mounted > manifest.maxMountedLayers) {
            errors.push(`${w} mounts ${mounted} layers, more than its own maxMountedLayers ${manifest.maxMountedLayers}`);
          }
        }
      }
    }
  }
  if (errors.length) return { ok: false, errors, files: [], bytes: 0 };

  /* ---- files: every referenced png at both scales ---- */
  const ship = ['layers.json', 'states.json'];
  let bytes = ship.reduce((n, f) => n + fs.statSync(path.join(dir, f)).size, 0);
  for (const [name, what] of files) {
    for (const [scale, px] of Object.entries(SCALES)) {
      const f = `${name}@${scale}.png`;
      const full = path.join(dir, f);
      let st;
      try {
        st = fs.lstatSync(full);
      } catch {
        errors.push(`${f} (${what}) is missing`);
        continue;
      }
      if (!st.isFile()) {
        errors.push(`${f} is not a regular file`);
        continue;
      }
      const errs = checkPng(fs.readFileSync(full), px);
      for (const e of errs) errors.push(`${f}: ${e}`);
      bytes += st.size;
      ship.push(f);
    }
  }
  if (!errors.length && bytes >= MAX_SKIN_BYTES) errors.push(`shipped layered bytes ${bytes} are not under ${MAX_SKIN_BYTES}`);
  if (errors.length) return { ok: false, errors, files: [], bytes };
  return {
    ok: true,
    errors,
    files: ship,
    bytes,
    info: { layers: base.layers.length, overlays: [...files.values()].filter((v) => v.startsWith('overlay')).length, pngs: ship.length - 2 },
  };
}
