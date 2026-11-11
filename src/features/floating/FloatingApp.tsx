import { useCallback, useEffect, useRef, useState, type CSSProperties, type PointerEvent } from 'react';
import { ipc } from '../../lib/ipc';
import { startWindowDrag } from '../../lib/ipc-tauri';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { Sprite } from '../../components/Sprite';
import { SPRITE_STATES, type SpriteState } from '../../components/spriteState';
import { actions, deriveSpriteState, useApp } from '../../state/app-store';
import { Wheel } from './Wheel';
import { dndPolicy, type WheelData, type WheelItem } from './wheel-items';
import './floating.css';

type Dock = 'none' | 'left' | 'right';
const DOCK_KEY = 'elf-dock';

/** pointer must REST on the sprite this long before the wheel opens; passing over does nothing */
const OPEN_DELAY_MS = 250;
/** grace after leaving sprite+wheel before it closes */
const CLOSE_DELAY_MS = 300;
/** close animation, then the window shrinks back */
const CLOSE_ANIM_MS = 160;

function params() {
  try {
    return new URLSearchParams(location.search);
  } catch {
    return new URLSearchParams();
  }
}

function readDock(): Dock {
  const q = params().get('dock');
  if (q === 'left' || q === 'right' || q === 'none') return q;
  try {
    const v = localStorage.getItem(DOCK_KEY);
    if (v === 'left' || v === 'right' || v === 'none') return v;
  } catch {
    /* ignore */
  }
  return 'none';
}

function forcedState(): SpriteState | null {
  const q = params().get('state');
  return SPRITE_STATES.includes(q as SpriteState) ? (q as SpriteState) : null;
}

const reducedMotion = () => {
  try {
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  } catch {
    return false;
  }
};

type Phase = 'closed' | 'opening' | 'open' | 'closing';

/**
 * Hover-to-open logic. Order matters: set_floating_expanded(true) is awaited BEFORE the wheel is
 * rendered, and set_floating_expanded(false) is sent AFTER the close animation.
 */
function useWheel() {
  const [phase, setPhase] = useState<Phase>('closed');
  const phaseRef = useRef<Phase>('closed');
  const openTimer = useRef<number | undefined>(undefined);
  const closeTimer = useRef<number | undefined>(undefined);
  const collapseTimer = useRef<number | undefined>(undefined);
  const cancelOpening = useRef(false);
  const pressed = useRef(false);
  // sprite centre relative to the window centre, as reported by the shell (edge-clamped window)
  const [offset, setOffset] = useState({ x: 0, y: 0 });

  const set = (p: Phase) => {
    phaseRef.current = p;
    setPhase(p);
  };
  const clearAll = () => {
    window.clearTimeout(openTimer.current);
    window.clearTimeout(closeTimer.current);
    openTimer.current = closeTimer.current = undefined;
  };

  const expand = async (expanded: boolean) => {
    try {
      const r = await ipc.invoke('set_floating_expanded', { expanded });
      setOffset(expanded ? { x: r.sprite_offset_x, y: r.sprite_offset_y } : { x: 0, y: 0 });
    } catch (e) {
      console.error('set_floating_expanded failed', e);
    }
  };

  const close = useCallback((immediate: boolean) => {
    clearAll();
    const p = phaseRef.current;
    if (p === 'opening') {
      cancelOpening.current = true; // the pending open will collapse the window when it resolves
      return;
    }
    if (p !== 'open') return;
    set('closing');
    window.clearTimeout(collapseTimer.current);
    collapseTimer.current = window.setTimeout(
      () => {
        void expand(false).then(() => set('closed'));
      },
      reducedMotion() ? 0 : immediate ? 100 : CLOSE_ANIM_MS,
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const doOpen = async () => {
    openTimer.current = undefined;
    if (pressed.current || phaseRef.current !== 'closed') return;
    cancelOpening.current = false;
    set('opening');
    await expand(true); // window grows first...
    if (cancelOpening.current) {
      await expand(false);
      set('closed');
      return;
    }
    set('open'); // ...then the wheel appears
  };

  /** pointer entered the sprite or a wheel item */
  const enter = (e?: PointerEvent) => {
    // a native window drag swallows the pointerup, so clear a stale press once no button is down
    if (e && e.buttons === 0) pressed.current = false;
    window.clearTimeout(closeTimer.current);
    closeTimer.current = undefined;
    const p = phaseRef.current;
    if (p === 'closing') {
      window.clearTimeout(collapseTimer.current);
      set('open');
      return;
    }
    if (p === 'opening') cancelOpening.current = false;
    if (p === 'closed' && !openTimer.current && !pressed.current && !(e && e.buttons !== 0)) {
      openTimer.current = window.setTimeout(() => void doOpen(), OPEN_DELAY_MS);
    }
  };

  /** pointer left the sprite or a wheel item */
  const leave = () => {
    window.clearTimeout(openTimer.current);
    openTimer.current = undefined;
    const p = phaseRef.current;
    if (p === 'open' || p === 'opening') {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = window.setTimeout(() => close(false), CLOSE_DELAY_MS);
    }
  };

  /** a press on the sprite may become a window drag: never open, and shut an open wheel at once */
  const press = () => {
    pressed.current = true;
    clearAll();
    close(true);
    const up = () => {
      pressed.current = false;
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', up);
    };
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', up);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close(true);
    };
    const onBlur = () => close(true);
    window.addEventListener('keydown', onKey);
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('blur', onBlur);
      clearAll();
      if (phaseRef.current !== 'closed') void ipc.invoke('set_floating_expanded', { expanded: false });
    };
  }, [close]);

  return { phase, phaseRef, offset, enter, leave, press, close };
}

const DRAG_THRESHOLD_PX = 4;

export function FloatingApp() {
  const live = useApp(deriveSpriteState);
  const forced = forcedState();
  const state = forced ?? live;
  const [dock, setDock] = useState<Dock>(readDock);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const wheel = useWheel();
  const dockTimer = useRef<number | undefined>(undefined);

  // press-and-move on the sprite drags the window; a press without movement stays a click
  const dragOrigin = useRef<{ x: number; y: number } | null>(null);
  const dragged = useRef(false);
  const onSpriteDown = (e: PointerEvent) => {
    wheel.press();
    dragged.current = false;
    dragOrigin.current = e.button === 0 ? { x: e.screenX, y: e.screenY } : null;
  };
  const onSpriteMove = (e: PointerEvent) => {
    wheel.enter(e);
    const o = dragOrigin.current;
    if (!o || e.buttons !== 1) {
      dragOrigin.current = null;
      return;
    }
    if (Math.hypot(e.screenX - o.x, e.screenY - o.y) < DRAG_THRESHOLD_PX) return;
    if (wheel.phaseRef.current !== 'closed') return; // wait until the window is back to its small size
    dragOrigin.current = null;
    dragged.current = true;
    void startWindowDrag().catch((err) => console.error('startDragging failed', err));
  };

  const suggestion = useApp((s) => s.today?.suggestions[0] ?? null);
  const tasks = useApp((s) => s.tasks);
  const today = useApp((s) => s.today);
  const observation = useApp((s) => s.observation);
  const waiting = Object.values(tasks).filter((x) => x.status === 'awaiting_user').length;

  useEffect(() => {
    try {
      localStorage.setItem(DOCK_KEY, dock);
    } catch {
      /* ignore */
    }
  }, [dock]);

  // Hint text: only for the two states that deserve a nudge, and never while do-not-disturb.
  let hintText: string | null = null;
  let hintKey = '';
  let hintTab: string = 'today';
  if (state === 'awaiting') {
    hintText = t.sprite.hintAwaiting(Math.max(waiting, 1));
    hintKey = `awaiting-${waiting}`;
    hintTab = 'tasks';
  } else if (state === 'suggestion') {
    hintText = suggestion?.title ?? '「繳交本月差旅報銷」今天要到期';
    hintKey = `sug-${suggestion?.id ?? 'preview'}`;
  }
  const showHint = !!hintText && dismissed !== hintKey && wheel.phase === 'closed';

  const openPanel = (tab: string | null) => void ipc.invoke('open_window', { label: 'panel', tab });
  const side = dock === 'right' ? 'right' : 'left';

  const data: WheelData = {
    tasks: Object.values(tasks),
    today,
    dnd: observation ? observation.status.dnd : null,
    now: Date.now(),
  };

  const choose = (item: WheelItem) => {
    wheel.close(true);
    const a = item.action;
    if (a.kind === 'open') void ipc.invoke('open_window', { label: a.window, tab: a.tab });
    else if (observation) void actions.setPolicy(dndPolicy(observation.policy)); // toggles in place
  };

  if (dock !== 'none') {
    // Docked: the visible strip is 12x48, the button (hit area) is 44 wide. Hovering it undocks the
    // sprite into its normal form after the same 250 ms rest; the wheel is then one more hover away.
    const peekState = state !== 'idle';
    return (
      <div className={`float float--dock float--${dock}`} data-state={state} data-testid="float-collapsed">
        <button
          className="dock"
          aria-label={`${t.sprite.expand}：${t.sprite.describe[state]}`}
          title={t.sprite.describe[state]}
          onClick={() => setDock('none')}
          onPointerEnter={(e) => {
            if (e.buttons === 0) dockTimer.current = window.setTimeout(() => setDock('none'), 250);
          }}
          onPointerLeave={() => window.clearTimeout(dockTimer.current)}
        >
          <Sprite state={state} variant="collapsed" title="" className="dock__strip" />
          {peekState && (
            <span className="dock__peek" aria-hidden="true">
              <Sprite state={state} size="100%" title="" />
            </span>
          )}
        </button>
      </div>
    );
  }

  return (
    <div className={`float float--${side} ${wheel.phase !== 'closed' ? 'float--wheel' : ''}`} data-state={state} data-testid="float-expanded" data-wheel={wheel.phase}>
      <div
        className="float__stage"
        style={{ '--dx': `${wheel.offset.x}px`, '--dy': `${wheel.offset.y}px` } as CSSProperties}
      >
      {(wheel.phase === 'open' || wheel.phase === 'closing') && (
        <Wheel data={data} closing={wheel.phase === 'closing'} onChoose={choose} onEnter={() => wheel.enter()} onLeave={wheel.leave} />
      )}

      <div className="float__main" data-tauri-drag-region>
        <button
          className="float__sprite"
          onClick={() => {
            if (dragged.current) {
              dragged.current = false; // the press became a drag: not a click
              return;
            }
            openPanel(state === 'awaiting' ? 'tasks' : state === 'suggestion' ? 'today' : null);
          }}
          aria-label={t.sprite.describe[state]}
          title={t.sprite.describe[state]}
          onPointerEnter={wheel.enter}
          onPointerMove={onSpriteMove} /* also re-arms the wheel if an enter event was missed */
          onPointerLeave={wheel.leave}
          onPointerDown={onSpriteDown}
        >
          <Sprite state={state} size="100%" title="" />
        </button>
        {wheel.phase === 'closed' && (
          <button
            className="btn btn--quiet btn--icon btn--small float__collapse"
            aria-label={t.sprite.collapse}
            title={t.sprite.collapse}
            onClick={() => setDock('left')}
          >
            <Icon name="chevron" style={{ transform: 'rotate(90deg)' }} />
          </button>
        )}

        {showHint && hintText && (
          <aside className="hint" role="status" key={hintKey}>
            <p className="hint__text">{hintText}</p>
            <div className="hint__actions">
              <button className="btn btn--primary btn--small" onClick={() => openPanel(hintTab)}>
                {t.sprite.hintOpen}
              </button>
              <button className="btn btn--quiet btn--small" onClick={() => setDismissed(hintKey)}>
                {t.sprite.hintLater}
              </button>
            </div>
          </aside>
        )}
      </div>

      </div>

      <p className="sr-only" role="status">
        {t.sprite.label[state]}
      </p>
    </div>
  );
}
