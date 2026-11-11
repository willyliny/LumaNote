/**
 * Types and the runtime parser for a skin's 2.5D layered set (layers.json + states.json).
 * scripts/layered-validate.mjs is the strict build-time twin of this file; this one only has to refuse
 * anything the renderer could not draw safely, so that the caller falls back to the flat SVG art.
 *
 * Contract: docs/briefs/astra-2p5d-night-letter-brief.zh-TW.md, docs/design/skins/LAYERED-*-DELIVERY.zh-TW.md.
 */
import { SKIN_STATES, type SkinState } from '../states';

export const MAX_MOUNTED_LAYERS = 8;
export const LAYERED_CANVAS = 384;
const SAFE_ID = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

export type BaseRole = 'shadow' | 'glow' | 'body' | 'highlight' | 'eyes-open' | 'eyes-closed' | 'accessory';
const BASE_ROLES: readonly BaseRole[] = ['shadow', 'glow', 'body', 'highlight', 'eyes-open', 'eyes-closed', 'accessory'];

export interface BaseLayer {
  id: string;
  /** file name without scale and extension: `<file>@1x.png` / `<file>@2x.png` */
  file: string;
  role: BaseRole;
}
export interface LayersJson {
  id: string;
  canvas: number;
  layers: BaseLayer[];
  /** rotation centre as fractions of the canvas */
  pivot: { x: number; y: number };
}
export interface StateOverlay {
  id: string;
  file: string;
  role: 'state';
}
export interface StateSpec {
  label: string;
  hiddenBaseLayers: string[];
  overlay: StateOverlay | null;
  eyes: 'open' | 'closed';
  blink: boolean;
  motion: 'idle' | 'none';
  composite: string;
}
export interface StatesJson {
  schemaVersion: 1;
  id: string;
  canvas: number;
  base: string;
  defaultState: SkinState;
  maxMountedLayers: number;
  states: Record<SkinState, StateSpec>;
}

export type ImageScale = 1 | 2;
/** Everything the renderer needs for one skin; present only when every referenced file exists. */
export interface LayeredArt {
  id: string;
  base: LayersJson;
  manifest: StatesJson;
  /** file (no scale/ext) -> URL at @1x and @2x */
  urls: Record<string, Record<ImageScale, string>>;
  /** distinct files of the base layers and of all state overlays, for preloading */
  files: string[];
}

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);
const safe = (v: unknown): v is string => typeof v === 'string' && v.length <= 48 && SAFE_ID.test(v);
const unit = (v: unknown): v is number => typeof v === 'number' && v >= 0 && v <= 1;

/**
 * @param lookup  resolves `<file>@<n>x.png` to a URL or undefined (from the bundled asset glob)
 * @returns the art, or a short reason string when the set must not be used
 */
export function parseLayered(
  skinId: string,
  rawBase: unknown,
  rawManifest: unknown,
  lookup: (file: string, scale: ImageScale) => string | undefined,
): LayeredArt | string {
  if (!isObj(rawBase) || !isObj(rawManifest)) return 'layers.json / states.json are not objects';
  if (rawBase.id !== skinId || rawManifest.id !== skinId) return 'id does not match the skin';
  if (rawBase.canvas !== LAYERED_CANVAS || rawManifest.canvas !== LAYERED_CANVAS) return 'canvas is not 384';
  if (rawManifest.schemaVersion !== 1 || rawManifest.base !== 'layers.json') return 'unsupported states.json';
  const pivot = rawBase.pivot;
  if (!isObj(pivot) || !unit(pivot.x) || !unit(pivot.y)) return 'bad pivot';
  const layers = rawBase.layers;
  if (!Array.isArray(layers) || layers.length === 0 || layers.length > MAX_MOUNTED_LAYERS) return 'bad layer list';
  const ids = new Set<string>();
  for (const l of layers) {
    if (!isObj(l) || !safe(l.id) || !safe(l.file) || !BASE_ROLES.includes(l.role as BaseRole)) return 'bad layer entry';
    if (ids.has(l.id)) return 'duplicate layer id';
    ids.add(l.id);
  }
  const roles = (layers as BaseLayer[]).map((l) => l.role);
  if (!roles.includes('body')) return 'no body layer';
  if (roles.includes('eyes-open') !== roles.includes('eyes-closed')) return 'eyes-open / eyes-closed must come as a pair';

  const max = rawManifest.maxMountedLayers;
  if (!Number.isInteger(max) || (max as number) < 1 || (max as number) > MAX_MOUNTED_LAYERS) return 'bad maxMountedLayers';
  const states = rawManifest.states;
  if (!isObj(states)) return 'no states';
  const files = new Set<string>((layers as BaseLayer[]).map((l) => l.file));
  for (const name of SKIN_STATES) {
    const s = states[name];
    if (!isObj(s)) return `state ${name} missing`;
    if (typeof s.label !== 'string') return `state ${name}: bad label`;
    if (!Array.isArray(s.hiddenBaseLayers) || s.hiddenBaseLayers.some((id) => typeof id !== 'string' || !ids.has(id))) {
      return `state ${name}: bad hiddenBaseLayers`;
    }
    if (s.eyes !== 'open' && s.eyes !== 'closed') return `state ${name}: bad eyes`;
    if (typeof s.blink !== 'boolean' || (s.eyes === 'closed' && s.blink)) return `state ${name}: bad blink`;
    if (s.motion !== 'idle' && s.motion !== 'none') return `state ${name}: bad motion`;
    let overlay = 0;
    if (s.overlay !== null) {
      const o = s.overlay;
      if (!isObj(o) || !safe(o.id) || !safe(o.file) || o.role !== 'state' || ids.has(o.id)) return `state ${name}: bad overlay`;
      overlay = 1;
      files.add(o.file);
    }
    const mounted = layers.length - new Set(s.hiddenBaseLayers as string[]).size + overlay;
    if (mounted > MAX_MOUNTED_LAYERS || mounted > (max as number)) return `state ${name} mounts ${mounted} layers`;
  }
  if (typeof rawManifest.defaultState !== 'string' || !SKIN_STATES.includes(rawManifest.defaultState as SkinState)) {
    return 'bad defaultState';
  }

  const urls: LayeredArt['urls'] = {};
  for (const f of files) {
    const u1 = lookup(f, 1);
    const u2 = lookup(f, 2);
    if (!u1 || !u2) return `${f} is missing at ${!u1 ? '@1x' : '@2x'}`;
    urls[f] = { 1: u1, 2: u2 };
  }
  return {
    id: skinId,
    base: rawBase as unknown as LayersJson,
    manifest: rawManifest as unknown as StatesJson,
    urls,
    files: [...files],
  };
}
