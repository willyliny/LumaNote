import type { CSSProperties, PointerEvent } from 'react';
import { Icon } from '../../components/Icon';
import { t } from '../../lib/strings';
import { WHEEL_ITEMS, WHEEL_LAYOUT, type WheelData, type WheelItem } from './wheel-items';
import { WHEEL_ITEM_H, WHEEL_ITEM_W, wheelSlot } from './layout';

/** Slots sit on an ellipse (wider than tall) around the 96 px sprite: radii and card size live in layout.ts. */
const pos = wheelSlot;

/**
 * Pointer-only radial menu. The floating window cannot take keyboard focus on Windows, so nothing
 * here asks for it: buttons are tabIndex -1 and pointer-down never moves focus.
 */
export function Wheel({
  data,
  closing,
  onChoose,
  onEnter,
  onLeave,
}: {
  data: WheelData;
  closing: boolean;
  onChoose: (item: WheelItem) => void;
  onEnter: () => void;
  onLeave: () => void;
}) {
  const ids = WHEEL_LAYOUT;
  return (
    <div className={`wheel ${closing ? 'wheel--closing' : ''}`} role="group" aria-label={t.wheel.label} data-testid="wheel" style={{ '--wheel-item-w': `${WHEEL_ITEM_W}px`, '--wheel-item-h': `${WHEEL_ITEM_H}px` } as CSSProperties}>
      {ids.map((id, i) => {
        const item = WHEEL_ITEMS[id];
        const v = item.value(data);
        const p = pos(i, ids.length);
        const isToggle = item.action.kind === 'toggle_dnd';
        return (
          <button
            key={id}
            type="button"
            tabIndex={-1}
            className={`wheel__item ${v.attention ? 'wheel__item--attn' : ''} ${v.pressed ? 'wheel__item--on' : ''}`}
            style={{ '--x': `${p.x}px`, '--y': `${p.y}px`, '--i': i } as CSSProperties}
            aria-pressed={isToggle ? !!v.pressed : undefined}
            aria-label={isToggle ? t.wheel.ariaDnd(!!v.pressed) : t.wheel.ariaOpen(item.label, v.text)}
            data-item={id}
            onPointerDown={(e: PointerEvent) => e.preventDefault() /* never take focus */}
            onPointerEnter={onEnter}
            onPointerLeave={onLeave}
            onClick={() => onChoose(item)}
          >
            <span className="wheel__head">
              <Icon name={v.icon ?? item.icon} />
              <span className="wheel__label">{item.label}</span>
            </span>
            <span className="wheel__value">{v.text}</span>
          </button>
        );
      })}
    </div>
  );
}
