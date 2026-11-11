import { useEffect, type CSSProperties } from 'react';
import { t } from '../lib/strings';
import type { SpriteState } from './spriteState';
import { APP_TO_SKIN_STATE, SKIN_STATES } from '../skins/states';
import type { CompleteSkin } from '../skins/types';
import { useActiveSkin } from '../state/appearance-store';
import './sprite.css';

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
}: {
  state: SpriteState;
  size?: number | string;
  /** accessible name; pass '' when a parent button already carries the name */
  title?: string;
  variant?: SpriteVariant;
  /** preview a specific skin (settings page); defaults to the active skin */
  skin?: CompleteSkin;
  className?: string;
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
  const style: CSSProperties | undefined = size === undefined ? undefined : { width: size, height: size };

  return (
    <img
      className={`sprite sprite--${variant} sprite--${state} ${className}`}
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
