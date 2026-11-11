import type { CSSProperties, PointerEvent } from 'react';
import { Icon } from '../../components/Icon';
import { t } from '../../lib/strings';
import { WHEEL_ITEMS, WHEEL_LAYOUT, type WheelData, type WheelItem } from './wheel-items';

/** Slots sit on an ellipse (wider than tall) so 84x56 cards never overlap: see floating.css. */
const RX = 100;
const RY = 92;

function pos(i: number, n: number) {
  const a = ((-90 + (360 / n) * i) * Math.PI) / 180;
  return { x: Math.round(RX * Math.cos(a)), y: Math.round(RY * Math.sin(a)) };
}

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
    <div className={`wheel ${closing ? 'wheel--closing' : ''}`} role="group" aria-label={t.wheel.label} data-testid="wheel">
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
