import { useRef, useState, type KeyboardEvent } from 'react';
import { t } from '../lib/strings';
import { Icon, type IconName } from '../components/Icon';
import { Sprite } from '../components/Sprite';
import { Toast } from '../components/Toast';
import { ipc } from '../lib/ipc';
import { deriveSpriteState, groupOf, refreshTask, selectJobs, syncAll, useApp } from '../state/app-store';
import { ChatView } from './chat/ChatView';
import { TasksView } from './tasks/TasksView';
import { TodayView } from './today/TodayView';
import './panel.css';

type Tab = 'chat' | 'tasks' | 'today';
const TABS: { id: Tab; icon: IconName }[] = [
  { id: 'chat', icon: 'chat' },
  { id: 'tasks', icon: 'tasks' },
  { id: 'today', icon: 'today' },
];

function initialTab(): Tab {
  const q = new URLSearchParams(location.search).get('tab');
  return q === 'tasks' || q === 'today' || q === 'chat' ? q : 'chat';
}

export function PanelApp() {
  const [tab, setTab] = useState<Tab>(initialTab);
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});
  const tasks = useApp((s) => s.tasks);
  const sprite = useApp(deriveSpriteState);
  const suggestions = useApp((s) => s.today?.suggestions.length ?? 0);
  const syncError = useApp((s) => s.syncError);

  const jobs = selectJobs(tasks);
  const waiting = jobs.filter((j) => groupOf(j.status) === 'waiting').length;
  const running = jobs.filter((j) => groupOf(j.status) === 'running').length;
  const summary = t.header.summary(running, waiting) || t.header.idle;
  const badge: Record<Tab, number> = { chat: 0, tasks: waiting, today: suggestions };

  const onKey = (e: KeyboardEvent) => {
    const i = TABS.findIndex((x) => x.id === tab);
    let n = i;
    if (e.key === 'ArrowRight') n = (i + 1) % TABS.length;
    else if (e.key === 'ArrowLeft') n = (i + TABS.length - 1) % TABS.length;
    else return;
    e.preventDefault();
    const next = TABS[n]!.id;
    setTab(next);
    refs.current[next]?.focus();
  };

  return (
    <div className="shell">
      <header className="shell__head">
        <div className="shell__sprite">
          <Sprite state={sprite} size="100%" variant="avatar" />
        </div>
        <div className="shell__title">
          <h1>{t.appName}</h1>
          <p aria-live="polite">{summary}</p>
        </div>
        <button
          className="btn btn--quiet btn--icon"
          aria-label={t.common.openSettings}
          title={t.common.openSettings}
          onClick={() => void ipc.invoke('open_window', { label: 'settings', tab: null })}
        >
          <Icon name="gear" />
        </button>
      </header>

      {syncError && (
        <div className="shell__stale" role="alert">
          <span>{t.connection.failed}</span>
          <button className="btn btn--small" onClick={() => void syncAll()}>
            {t.connection.retry}
          </button>
        </div>
      )}

      <div className="tabs" role="tablist" aria-label={t.appName} onKeyDown={onKey}>
        {TABS.map(({ id, icon }) => (
          <button
            key={id}
            ref={(el) => {
              refs.current[id] = el;
            }}
            role="tab"
            id={`tab-${id}`}
            aria-selected={tab === id}
            aria-controls={`pane-${id}`}
            tabIndex={tab === id ? 0 : -1}
            className="tabs__tab"
            onClick={() => {
              setTab(id);
              if (id === 'tasks') for (const j of jobs) if (groupOf(j.status) !== 'done') void refreshTask(j.id);
            }}
          >
            <Icon name={icon} />
            {t.tabs[id]}
            {badge[id] > 0 && <span className="tabs__badge" aria-label={`${badge[id]}`}>{badge[id]}</span>}
          </button>
        ))}
      </div>

      <main className="shell__main" role="tabpanel" id={`pane-${tab}`} aria-labelledby={`tab-${tab}`}>
        {tab === 'chat' && <ChatView onOpenTasks={() => setTab('tasks')} />}
        {tab === 'tasks' && (
          <div className="shell__scroll">
            <TasksView />
          </div>
        )}
        {tab === 'today' && (
          <div className="shell__scroll">
            <TodayView />
          </div>
        )}
      </main>
      <Toast />
    </div>
  );
}
