import { useCallback, useEffect, useState } from 'react';
import { ipc } from '../../lib/ipc';
import type { Memory, MemoryType } from '../../lib/ipc-types';
import { formatWhen } from '../../lib/format';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { notify, reportError } from '../../state/app-store';

const TYPES: MemoryType[] = ['explicit_preference', 'task_fact', 'short_term', 'inferred_habit'];

export function MemorySection() {
  const [items, setItems] = useState<Memory[] | null>(null);
  const [filter, setFilter] = useState<MemoryType | null>(null);
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setItems(await ipc.invoke('list_memories', { type: null }));
    } catch (e) {
      reportError(e);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  if (!items) return <p className="subtle">{t.common.loading}</p>;
  const shown = items.filter((m) => !filter || m.type === filter);

  const saveEdit = async () => {
    if (!editing || !editing.text.trim()) return;
    try {
      await ipc.invoke('correct_memory', { id: editing.id, content: editing.text.trim() });
      notify('info', t.memory.corrected);
      setEditing(null);
      await load();
    } catch (e) {
      reportError(e);
    }
  };
  const del = async (id: string) => {
    try {
      const r = await ipc.invoke('delete_memory', { id });
      notify('info', t.memory.deleted(r.derived_invalidated));
      setConfirming(null);
      await load();
    } catch (e) {
      reportError(e);
    }
  };

  return (
    <section className="sec">
      <h2>{t.memory.heading}</h2>
      <p className="sec__intro">{t.memory.intro}</p>

      <div className="filters" role="group" aria-label={t.memory.heading}>
        <button className={`filter ${filter === null ? 'filter--on' : ''}`} aria-pressed={filter === null} onClick={() => setFilter(null)}>
          {t.memory.filterAll}
          <span className="filter__n">{items.length}</span>
        </button>
        {TYPES.map((ty) => (
          <button key={ty} className={`filter ${filter === ty ? 'filter--on' : ''}`} aria-pressed={filter === ty} onClick={() => setFilter(ty)}>
            {t.memory.types[ty]}
            <span className="filter__n">{items.filter((m) => m.type === ty).length}</span>
          </button>
        ))}
      </div>

      {shown.length === 0 ? (
        <p className="subtle">{t.memory.empty}</p>
      ) : (
        <ul className="memories">
          {shown.map((m) => (
            <li key={m.id} className="memory" data-type={m.type}>
              <header className="memory__head">
                <span className={`badge memory__type memory__type--${m.type}`}>
                  <Icon name={m.type === 'explicit_preference' ? 'check' : m.type === 'inferred_habit' ? 'question' : m.type === 'short_term' ? 'clock' : 'file'} />
                  {t.memory.types[m.type]}
                </span>
                {m.confidence !== null && <span className="subtle">{t.memory.confidence(Math.round(m.confidence * 100))}</span>}
                {m.expires_at && <span className="subtle memory__expiry">{t.memory.expires(formatWhen(m.expires_at))}</span>}
              </header>

              {editing?.id === m.id ? (
                <div className="memory__edit">
                  <label className="field">
                    <span className="field__label">{t.memory.correctLabel}</span>
                    <textarea
                      className="textarea"
                      rows={2}
                      value={editing.text}
                      onChange={(e) => setEditing({ id: m.id, text: e.target.value })}
                    />
                  </label>
                  <div className="btnrow">
                    <button className="btn btn--primary btn--small" onClick={saveEdit}>
                      {t.memory.saveCorrection}
                    </button>
                    <button className="btn btn--small" onClick={() => setEditing(null)}>
                      {t.common.cancel}
                    </button>
                  </div>
                </div>
              ) : (
                <p className="memory__content">{m.content}</p>
              )}

              <dl className="memory__meta">
                <div>
                  <dt>{t.memory.source}</dt>
                  <dd>{m.sources.map((s) => s.label).join('、')}</dd>
                </div>
                {m.evidence_note && (
                  <div>
                    <dt>{t.memory.evidence}</dt>
                    <dd>{m.evidence_note}</dd>
                  </div>
                )}
              </dl>

              {editing?.id !== m.id && (
                <div className="btnrow">
                  <button className="btn btn--small" onClick={() => setEditing({ id: m.id, text: m.content })}>
                    <Icon name="pencil" />
                    {t.memory.correct}
                  </button>
                  {confirming === m.id ? (
                    <span className="profile__confirm" role="alert">
                      <span>{t.memory.confirmDelete}</span>
                      <button className="btn btn--danger btn--small" onClick={() => del(m.id)}>
                        {t.common.confirmDelete}
                      </button>
                      <button className="btn btn--quiet btn--small" onClick={() => setConfirming(null)}>
                        {t.common.cancel}
                      </button>
                    </span>
                  ) : (
                    <button className="btn btn--quiet btn--small" onClick={() => setConfirming(m.id)}>
                      <Icon name="trash" />
                      {t.common.delete}
                    </button>
                  )}
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
