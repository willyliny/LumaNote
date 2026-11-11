/**
 * Which layers are mounted, and how they look, for one skin state.
 * A TypeScript reimplementation of the designer's reference resolver
 * (docs/design/skins/layered-state-resolver.mjs): base layers minus the state's hidden ones, plus the
 * overlay; eyes follow the state; unknown states fall back to the manifest default. Pure and scale-free:
 * the component turns `file` into a URL.
 */
import type { SkinState } from '../states';
import type { BaseRole, LayeredArt } from './schema';

export type FrameRole = BaseRole | 'state';

export interface FrameLayer {
  id: string;
  file: string;
  role: FrameRole;
  /** 1 for visible, 0 for the eye layer that is currently off */
  on: 0 | 1;
  /** true for layers that cross-fade when a state adds or removes them (glow, state marker) */
  fade: boolean;
  /** position in the base list (overlays sort after every base layer) */
  order: number;
}

export interface Frame {
  state: SkinState;
  label: string;
  pivot: { x: number; y: number };
  layers: FrameLayer[];
  /** blinking allowed (open eyes in a state that permits it) */
  blink: boolean;
  /** 'none': floating and blinking are off for this state */
  motion: 'idle' | 'none';
}

export function resolveFrame(art: LayeredArt, state: SkinState): Frame {
  const { base, manifest } = art;
  const selected: SkinState = Object.hasOwn(manifest.states, state) ? state : manifest.defaultState;
  const spec = manifest.states[selected];
  const hidden = new Set(spec.hiddenBaseLayers);
  const layers: FrameLayer[] = [];
  base.layers.forEach((l, i) => {
    if (hidden.has(l.id)) return;
    layers.push({
      id: l.id,
      file: l.file,
      role: l.role,
      on: l.role === 'eyes-open' ? (spec.eyes === 'open' ? 1 : 0) : l.role === 'eyes-closed' ? (spec.eyes === 'closed' ? 1 : 0) : 1,
      fade: l.role === 'glow',
      order: i,
    });
  });
  if (spec.overlay) {
    layers.push({ id: spec.overlay.id, file: spec.overlay.file, role: 'state', on: 1, fade: true, order: base.layers.length });
  }
  return {
    state: selected,
    label: spec.label,
    pivot: base.pivot,
    layers,
    blink: spec.blink && spec.eyes === 'open',
    motion: spec.motion,
  };
}
