import { useEffect, useState } from 'react';
import { ipc } from '../../lib/ipc';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { Sprite } from '../../components/Sprite';
import { SPRITE_STATES, type SpriteState } from '../../components/spriteState';
import { deriveSpriteState, useApp } from '../../state/app-store';
import './floating.css';

type Dock = 'none' | 'left' | 'right';
const DOCK_KEY = 'elf-dock';

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

export function FloatingApp() {
  const live = useApp(deriveSpriteState);
  const forced = forcedState();
  const state = forced ?? live;
  const [dock, setDock] = useState<Dock>(readDock);
  const [dismissed, setDismissed] = useState<string | null>(null);

  const suggestion = useApp((s) => s.today?.suggestions[0] ?? null);
  const waiting = useApp((s) => Object.values(s.tasks).filter((x) => x.status === 'awaiting_user').length);

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
  const showHint = !!hintText && dismissed !== hintKey;

  const openPanel = (tab: string | null) => void ipc.invoke('open_window', { label: 'panel', tab });
  const side = dock === 'right' ? 'right' : 'left';

  if (dock !== 'none') {
    return (
      <div className={`float float--dock float--${dock}`} data-state={state} data-testid="float-collapsed">
        <button
          className="dockstrip"
          aria-label={`${t.sprite.expand}：${t.sprite.describe[state]}`}
          title={t.sprite.describe[state]}
          onClick={() => setDock('none')}
        >
          <span className="dockstrip__sprite">
            <Sprite state={state} size="100%" title="" />
          </span>
        </button>
      </div>
    );
  }

  return (
    <div className={`float float--${side}`} data-state={state} data-testid="float-expanded">
      <div className="float__main" data-tauri-drag-region>
        <button
          className="float__sprite"
          onClick={() => openPanel(state === 'awaiting' ? 'tasks' : state === 'suggestion' ? 'today' : null)}
          aria-label={t.sprite.describe[state]}
          title={t.sprite.describe[state]}
        >
          <Sprite state={state} size="100%" title="" />
        </button>
        <button
          className="btn btn--quiet btn--icon btn--small float__collapse"
          aria-label={t.sprite.collapse}
          title={t.sprite.collapse}
          onClick={() => setDock('left')}
        >
          <Icon name="chevron" style={{ transform: 'rotate(90deg)' }} />
        </button>
      </div>

      <p className="sr-only" role="status">
        {t.sprite.label[state]}
      </p>

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
  );
}
