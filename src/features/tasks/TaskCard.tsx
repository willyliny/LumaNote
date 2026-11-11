import { useState } from 'react';
import type { Task } from '../../lib/ipc-types';
import { formatRelative } from '../../lib/format';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { StatusBadge } from '../../components/StatusBadge';
import { ApprovalCard } from '../../components/ApprovalCard';
import { ArtifactList } from '../chat/ArtifactList';
import { actions } from '../../state/app-store';

const COLLAPSED_STEPS = 3;

export function TaskCard({ task }: { task: Task }) {
  const [all, setAll] = useState(false);
  const [busy, setBusy] = useState(false);
  const steps = task.steps;
  const shown = all ? steps : steps.slice(-COLLAPSED_STEPS);
  const hidden = steps.length - shown.length;

  const act = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await fn();
    } finally {
      setBusy(false);
    }
  };

  const s = task.status;
  const canCancel = ['queued', 'running', 'awaiting_user', 'paused', 'uncertain'].includes(s);
  const canRetry = (s === 'failed' && task.error?.retryable !== false) || s === 'cancelled';
  const needs = task.needs_user;
  const showError = !!task.error && (s === 'failed' || s === 'cancelled' || s === 'uncertain');
  const showNeeds = (!!needs && !task.pending_approval) || showError;

  return (
    <article className={`task task--${s}`} data-task={task.id} aria-label={task.intent}>
      <header className="task__head">
        <StatusBadge status={s} />
        <time className="subtle" dateTime={task.updated_at}>
          {formatRelative(task.updated_at)}
        </time>
      </header>
      <h3 className="task__title">{task.intent}</h3>

      {task.tainted_by_external && <p className="task__notice">{t.tasks.externalNotice}</p>}

      <section className="task__section">
        <h4>{t.tasks.done}</h4>
        {steps.length === 0 ? (
          <p className="subtle">{t.tasks.noSteps}</p>
        ) : (
          <>
            <ol className="steps">
              {shown.map((st) => (
                <li key={st.seq} className={`step step--${st.status}`}>
                  <Icon
                    name={st.status === 'succeeded' ? 'check' : st.status === 'failed' ? 'x' : 'spin'}
                    className={st.status === 'started' && !['awaiting_user', 'paused', 'uncertain', 'failed', 'cancelled'].includes(s) ? 'spin' : ''}
                  />
                  <span className="step__title">{st.title}</span>
                  <span className="step__tag">
                    {st.status === 'started' && s === 'awaiting_user'
                      ? t.status.awaiting_user
                      : st.status === 'started'
                        ? t.tasks.stepRunning
                        : st.status === 'failed'
                          ? t.tasks.stepFailed
                          : t.tasks.sideEffect[st.side_effect]}
                  </span>
                </li>
              ))}
            </ol>
            {steps.length > COLLAPSED_STEPS && (
              <button className="btn btn--quiet btn--small" onClick={() => setAll((v) => !v)}>
                {all ? t.tasks.hideSteps : t.tasks.showAllSteps(steps.length)}
              </button>
            )}
            {hidden > 0 && <span className="sr-only">{hidden}</span>}
          </>
        )}
      </section>

      <section className="task__section">
        <h4>{t.tasks.results}</h4>
        {task.artifacts.length === 0 ? (
          <p className="subtle">{t.tasks.noResults}</p>
        ) : (
          <ArtifactList artifacts={task.artifacts} />
        )}
      </section>

      {showNeeds && (
        <section className="task__section task__needs">
          <h4>
            <Icon name="alert" />
            {t.tasks.needsYou}
          </h4>
          {needs && !task.pending_approval && <p>{needs.summary}</p>}
          {needs?.kind === 'resume' && (
            <p className="subtle">{t.tasks.resumeReason[needs.reason]}</p>
          )}
          {showError && task.error && (
            <div className="task__error">
              <p className="task__error-title">{t.errors.title[task.error.kind]}</p>
              <p>{t.errors.hint[task.error.kind]}</p>
              <p className="subtle mono">{task.error.message}</p>
            </div>
          )}
        </section>
      )}

      {task.pending_approval && (
        <ApprovalCard
          approval={task.pending_approval}
          onRespond={(d) => actions.respondApproval(task.pending_approval!.approval_id, d)}
        />
      )}

      <footer className="task__foot">
        {task.model && (
          <span className="subtle task__model">
            {t.tasks.model}：{task.model.provider} ／ {task.model.model_id}
          </span>
        )}
        <div className="task__actions">
          {s === 'paused' && (
            <button className="btn btn--primary btn--small" disabled={busy} onClick={() => act(() => actions.resume(task.id))}>
              {t.tasks.resume}
            </button>
          )}
          {s === 'uncertain' && (
            <>
              <button className="btn btn--small" disabled={busy} onClick={() => act(() => actions.resolveUncertain(task.id, 'completed'))}>
                {t.tasks.confirmDone}
              </button>
              <button className="btn btn--small" disabled={busy} onClick={() => act(() => actions.resolveUncertain(task.id, 'failed'))}>
                {t.tasks.confirmFailed}
              </button>
            </>
          )}
          {canRetry && (
            <button className="btn btn--small" disabled={busy} onClick={() => act(() => actions.retry(task.id))}>
              <Icon name="spin" />
              {t.tasks.retry}
            </button>
          )}
          {canCancel && (
            <button className="btn btn--quiet btn--small" disabled={busy} onClick={() => act(() => actions.cancel(task.id))}>
              {t.tasks.cancel}
            </button>
          )}
        </div>
      </footer>
    </article>
  );
}
