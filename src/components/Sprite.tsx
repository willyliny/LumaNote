import { useEffect, type CSSProperties } from 'react';
import { t } from '../lib/strings';
import type { SpriteState } from './spriteState';
import { APP_TO_SKIN_STATE, SKIN_STATES } from '../skins/states';
import type { CompleteSkin } from '../skins/types';
import { useActiveSkin } from '../state/appearance-store';
import { LayeredSprite } from './LayeredSprite';
import { useLayeredStatus } from './layeredLoader';
import { pickScale, useDevicePixelRatio, useReducedMotion } from './spriteRuntime';
import './sprite.css';

/** Below this rendered size (panel avatar, docked strip, 48 px lists) the flat SVG is used: the 2.5D art is for >= 64. */
export const MIN_LAYERED_PX = 64;

function cssPx(size: number | string | undefined): number | null {
  if (typeof size === 'number') return size;
  const m = typeof size === 'string' ? /^(\d+(?:\.\d+)?)px$/.exec(size) : null;
  return m ? Number(m[1]) : null; // '100%' etc.: unknown, so flat
}

/**
 * Renders the active skin's art for a state as an <img> (never inlined), so SVG content can neither
 * execute nor restyle the page. Which file belongs to which state is decided in skins/states.ts.
 * A state with no file is never replaced by another skin's art: complete skins always have all six.
 */
export type SpriteVariant = 'state' | 'avatar' | 'collapsed';

const preloaded = new Set<string>();
function preload(skin: CompleteSkin) {
  const urls = [...SKIN_STATES.map((s) => skin.assets.states[s]), skin.assets.collapsed, skin.assets.avatar];
  for (const u of urls) {
    if (preloaded.has(u)) continue;
    preloaded.add(u);
    const img = new Image();
    img.decoding = 'async';
    img.src = u;
  }
}

export function Sprite({
  state,
  size,
  title,
  variant = 'state',
  skin: skinProp,
  className = '',
  animate = true,
  interactive = false,
  plain = false,
}: {
  state: SpriteState;
  size?: number | string;
  /** accessible name; pass '' when a parent button already carries the name */
  title?: string;
  variant?: SpriteVariant;
  /** preview a specific skin (settings page); defaults to the active skin */
  skin?: CompleteSkin;
  className?: string;
  /** 2.5D art only: false renders a still picture (no timers, listeners or animation) */
  animate?: boolean;
  /** 2.5D art only: tilt towards the pointer / with a drag (the floating sprite) */
  interactive?: boolean;
  /** flat art only: no drop-shadow/halo filter (for cards that sit on their own surface) */
  plain?: boolean;
}) {
  const active = useActiveSkin();
  const skin = skinProp ?? active;
  useEffect(() => preload(skin), [skin]);

  const src =
    variant === 'avatar'
      ? skin.assets.avatar
      : variant === 'collapsed'
        ? skin.assets.collapsed
        : skin.assets.states[APP_TO_SKIN_STATE[state]];
  const name = title ?? t.sprite.label[state];

  // 2.5D art: skin has a valid layered set, the state sprite is >= 64 CSS px, reduced motion is off.
  // Loading shows nothing (a flat flash would swap art); a load failure falls back to the flat art below.
  const reduced = useReducedMotion();
  const dpr = useDevicePixelRatio();
  const px = cssPx(size);
  const wantLayered = variant === 'state' && !!skin.layered && !reduced && px !== null && px >= MIN_LAYERED_PX;
  const layered = useLayeredStatus(wantLayered ? skin.layered : null, px ? pickScale(px, dpr) : 1);
  if (wantLayered && skin.layered && px !== null && layered.status !== 'failed') {
    if (layered.status === 'loading') {
      return <span className={`lsprite lsprite--loading ${className}`} style={{ width: px, height: px }} data-skin={skin.id} data-state={APP_TO_SKIN_STATE[state]} />;
    }
    return (
      <LayeredSprite
        art={skin.layered}
        skinState={APP_TO_SKIN_STATE[state]}
        size={px}
        scale={layered.scale}
        animate={animate}
        interactive={interactive}
        name={name}
        className={className}
      />
    );
  }

  const style: CSSProperties | undefined = size === undefined ? undefined : { width: size, height: size };

  return (
    <img
      className={`sprite sprite--${variant} sprite--${state} ${plain ? 'sprite--plain' : ''} ${className}`}
      src={src}
      alt={name}
      aria-hidden={name === '' ? true : undefined}
      title={name === '' ? undefined : name}
      draggable={false}
      decoding="async"
      style={variant === 'collapsed' ? undefined : style}
      data-skin={skin.id}
      data-state={state}
    />
  );
}
