import { formatTime, formatWhen } from '../../lib/format';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { Markdown } from '../../components/Markdown';
import { actions, useApp } from '../../state/app-store';
import { SuggestionCard } from './SuggestionCard';
import './today.css';

export function TodayView() {
  const today = useApp((s) => s.today);
  if (!today) return <p className="today__loading subtle">{t.common.loading}</p>;
  const todos = [...today.todos].sort((a, b) => Number(a.done) - Number(b.done));

  return (
    <div className="today">
      <section className="tsec" aria-labelledby="t-sug">
        <h2 id="t-sug">{t.today.suggestions}</h2>
        {today.suggestions.length === 0 ? (
          <p className="tsec__empty">{t.today.emptySuggestions}</p>
        ) : (
          today.suggestions.map((s) => <SuggestionCard key={s.id} s={s} />)
        )}
      </section>

      {today.daily_summary && (
        <section className="tsec" aria-labelledby="t-sum">
          <h2 id="t-sum">{t.today.summary}</h2>
          <div className="card tsec__card">
            <Markdown text={today.daily_summary.body} />
            <p className="subtle">{t.today.summarySource(today.daily_summary.source_ids.length)}</p>
          </div>
        </section>
      )}

      <section className="tsec" aria-labelledby="t-todo">
        <h2 id="t-todo">{t.today.todos}</h2>
        {todos.length === 0 ? (
          <p className="tsec__empty">{t.today.emptyTodos}</p>
        ) : (
          <ul className="list">
            {todos.map((td) => (
              <li key={td.id} className={`todo ${td.done ? 'todo--done' : ''}`}>
                <label className="todo__label">
                  <input
                    type="checkbox"
                    className="todo__check"
                    checked={td.done}
                    aria-label={td.done ? t.today.markUndone(td.title) : t.today.markDone(td.title)}
                    onChange={(e) => actions.toggleTodo(td.id, e.target.checked)}
                  />
                  <span className="todo__text">
                    <span className="todo__title">{td.title}</span>
                    <span className="todo__due">
                      {td.due_uncertain ? (
                        <span className="badge badge--warn badge--plain">
                          <Icon name="question" />
                          {t.today.dueUncertain}
                        </span>
                      ) : td.due_at ? (
                        <span className="subtle">{t.today.due(formatWhen(td.due_at))}</span>
                      ) : (
                        <span className="subtle">{t.today.noDue}</span>
                      )}
                    </span>
                  </span>
                </label>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="tsec" aria-labelledby="t-agenda">
        <h2 id="t-agenda">{t.today.agenda}</h2>
        {today.agenda.length === 0 ? (
          <p className="tsec__empty">{t.today.emptyAgenda}</p>
        ) : (
          <ul className="list">
            {today.agenda.map((a) => (
              <li key={a.id} className="row">
                <span className="row__time">
                  {formatTime(a.start_at)}
                  <span className="subtle">–{formatTime(a.end_at)}</span>
                </span>
                <span>{a.title}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="tsec" aria-labelledby="t-rem">
        <h2 id="t-rem">{t.today.reminders}</h2>
        {today.reminders.length === 0 ? (
          <p className="tsec__empty">{t.today.emptyReminders}</p>
        ) : (
          <ul className="list">
            {today.reminders.map((r) => (
              <li key={r.id} className="row row--stack">
                <span className="row__time">
                  <Icon name="clock" />
                  {formatWhen(r.next_run_at, r.timezone)}
                </span>
                <span className="row__main">
                  {r.title}
                  {r.recurrence && <span className="badge badge--plain">{t.today.recurring}</span>}
                </span>
                <span className="subtle row__note">{t.today.reminderRule(r.original_rule, r.timezone)}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
