import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import type { SkinState } from '../skins/states';
import type { ImageScale, LayeredArt } from '../skins/layered/schema';
import { resolveFrame, type FrameLayer } from '../skins/layered/resolver';
import { markLayeredFailed } from './layeredLoader';
import { useSpritesAwake, wakeSprites } from './spriteRuntime';
import './layered-sprite.css';

/**
 * The 2.5D sprite: the resolved layers as stacked <img> in ONE square box (same origin, no per-layer
 * positioning). All motion is CSS transform/translate/scale/opacity (see layered-sprite.css); the only
 * JS is a blink timer and pointer handlers that write two CSS variables. No filters, no canvas, no rAF loop.
 *
 *  - float: body-role layers (+ eyes, highlight) together, accessory lags, shadow and glow counter-breathe
 *  - blink: random 4-7 s, 150 ms, swaps the eyes-open / eyes-closed layers; only when the state allows it
 *  - tilt (interactive only): up to 4 deg around the manifest pivot while the pointer is near, or leaning
 *    with the movement while a press is dragging
 *  - state changes: the layer set is re-resolved; persistent layers keep their DOM nodes (body never
 *    remounts or moves), glow and state marker cross-fade for FADE_MS
 */
const FADE_MS = 140;
const BLINK_MS = 150;
const MAX_TILT_DEG = 4;
/** pointer farther than this many sprite half-widths from the centre is "not near": rest */
const NEAR_RADIUS = 2.2;
/** a drag lean (no more pointer events arrive once the OS takes over the drag) fades after this long */
const LEAN_HOLD_MS = 1200;

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

export function LayeredSprite({
  art,
  skinState,
  size,
  scale,
  animate,
  interactive = false,
  name,
  className = '',
}: {
  art: LayeredArt;
  skinState: SkinState;
  /** CSS px of the square box */
  size: number;
  scale: ImageScale;
  /** false: static picture (settings grid); no timers, listeners or animations */
  animate: boolean;
  /** tilt towards the pointer / with a drag (floating sprite only) */
  interactive?: boolean;
  /** accessible name; '' when a parent already carries it */
  name: string;
  className?: string;
}) {
  const frame = useMemo(() => resolveFrame(art, skinState), [art, skinState]);
  const awake = useSpritesAwake();
  const rootRef = useRef<HTMLDivElement>(null);

  /* ---- keep fading layers mounted for FADE_MS (derived during render, so there is no frame without them) ---- */
  const [shown, setShown] = useState({ frame, leaving: [] as FrameLayer[] });
  if (shown.frame !== frame) {
    const keep = new Set(frame.layers.map((l) => l.id));
    setShown({ frame, leaving: shown.frame.layers.filter((l) => l.fade && !keep.has(l.id)) });
  }
  useEffect(() => {
    if (shown.leaving.length === 0) return;
    const t = window.setTimeout(() => setShown((s) => ({ ...s, leaving: [] })), FADE_MS + 20);
    return () => window.clearTimeout(t);
  }, [shown.leaving]);

  // a state change wakes a sleeping sprite (the new state must be seen)
  const firstRun = useRef(true);
  useEffect(() => {
    if (firstRun.current) {
      firstRun.current = false;
      return;
    }
    wakeSprites();
  }, [skinState]);

  /* ---- blink: one timer chain per state; cleared before the next state's chain starts ---- */
  const canBlink = animate && awake && frame.blink;
  useEffect(() => {
    const root = rootRef.current;
    if (!canBlink || !root) return;
    let wait: number | undefined;
    let shut: number | undefined;
    const schedule = () => {
      wait = window.setTimeout(
        () => {
          root.dataset.blinking = '1';
          shut = window.setTimeout(() => {
            delete root.dataset.blinking;
            schedule();
          }, BLINK_MS);
        },
        4000 + Math.random() * 3000,
      );
    };
    schedule();
    return () => {
      window.clearTimeout(wait);
      window.clearTimeout(shut);
      delete root.dataset.blinking;
    };
  }, [canBlink]);

  /* ---- tilt ---- */
  const canTilt = animate && interactive && frame.motion === 'idle';
  useEffect(() => {
    const root = rootRef.current;
    if (!canTilt || !root) return;
    let pressed = false;
    let lean = 0;
    let leanTimer: number | undefined;
    const apply = (deg: number, hx: number, hy: number) => {
      root.style.setProperty('--tilt', deg.toFixed(2));
      root.style.setProperty('--hx', `${hx.toFixed(2)}px`);
      root.style.setProperty('--hy', `${hy.toFixed(2)}px`);
    };
    const rest = () => apply(0, 0, 0);
    const towards = (e: PointerEvent) => {
      const r = root.getBoundingClientRect();
      const dx = (e.clientX - (r.left + r.width / 2)) / (r.width / 2);
      const dy = (e.clientY - (r.top + r.height / 2)) / (r.height / 2);
      if (Math.hypot(dx, dy) > NEAR_RADIUS) return rest();
      // lean towards the pointer; the highlight slides the other way (a reflection stays put)
      apply(clamp(dx, -1, 1) * MAX_TILT_DEG, clamp(dx, -1, 1) * -1.5, clamp(dy, -1, 1) * -1.5);
    };
    const onMove = (e: PointerEvent) => {
      if (pressed && (e.buttons & 1) === 1) {
        // dragging: lean in the direction of movement, hold it a moment (the OS drag swallows later events)
        if (Math.abs(e.movementX) > 0) lean = clamp(lean * 0.5 + e.movementX * 0.9, -MAX_TILT_DEG, MAX_TILT_DEG);
        apply(lean, -lean * 0.4, 0);
        window.clearTimeout(leanTimer);
        leanTimer = window.setTimeout(rest, LEAN_HOLD_MS);
        return;
      }
      towards(e);
    };
    const onDown = () => {
      pressed = true;
      lean = 0;
    };
    const onUp = (e: PointerEvent) => {
      pressed = false;
      window.clearTimeout(leanTimer);
      towards(e);
    };
    const onLeave = () => {
      window.clearTimeout(leanTimer);
      rest();
    };
    window.addEventListener('pointermove', onMove, { passive: true });
    window.addEventListener('pointerdown', onDown, { passive: true });
    window.addEventListener('pointerup', onUp, { passive: true });
    window.addEventListener('pointercancel', onLeave, { passive: true });
    document.documentElement.addEventListener('pointerleave', onLeave, { passive: true });
    return () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerdown', onDown);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onLeave);
      document.documentElement.removeEventListener('pointerleave', onLeave);
      window.clearTimeout(leanTimer);
      rest();
    };
  }, [canTilt]);

  /* ---- render list: base order, overlays last; leaving layers keep their slot ---- */
  const list = [...frame.layers.map((l) => ({ l, out: false })), ...shown.leaving.map((l) => ({ l, out: true }))];
  list.sort((a, b) => a.l.order - b.l.order || Number(a.out) - Number(b.out));

  const src = (file: string) => art.urls[file]?.[scale];
  const renderLayer = ({ l, out }: { l: FrameLayer; out: boolean }) => {
    const img = (
      <img
        key={l.id}
        className={`l l--${l.role}`}
        data-layer={l.id}
        data-on={l.on}
        src={src(l.file)}
        alt=""
        draggable={false}
        decoding="async"
        onError={() => markLayeredFailed(art, `${l.file} did not load`)}
      />
    );
    // glow and state markers cross-fade; the wrapper owns the fade so the img can keep its own breathing
    return l.fade ? (
      <span key={l.id} className={`l-fade ${out ? 'l-fade--out' : 'l-fade--in'}`}>
        {img}
      </span>
    ) : (
      img
    );
  };
  const shadows = list.filter((x) => x.l.role === 'shadow');
  const rest = list.filter((x) => x.l.role !== 'shadow');

  const style = {
    width: size,
    height: size,
    '--px': `${art.base.pivot.x * 100}%`,
    '--py': `${art.base.pivot.y * 100}%`,
  } as CSSProperties;

  return (
    <div
      ref={rootRef}
      className={`lsprite ${className}`}
      style={style}
      role={name === '' ? undefined : 'img'}
      aria-label={name === '' ? undefined : name}
      aria-hidden={name === '' ? true : undefined}
      title={name === '' ? undefined : name}
      data-skin={art.id}
      data-state={skinState}
      data-motion={frame.motion}
      data-animate={animate ? '1' : '0'}
      data-asleep={awake ? '0' : '1'}
      data-scale={scale}
    >
      {/* the ground shadow stays level; everything else leans together */}
      {shadows.map(renderLayer)}
      <div className="lsprite__rig">{rest.map(renderLayer)}</div>
    </div>
  );
}
