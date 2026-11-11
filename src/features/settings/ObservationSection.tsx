import { useState } from 'react';
import type { Exclusion, ObservationPolicy, ObservationSourceId } from '../../lib/ipc-types';
import { formatRelative, formatWhen } from '../../lib/format';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { Toggle } from '../../components/Toggle';
import { actions, useApp } from '../../state/app-store';
import { useEffect } from 'react';
import { ipc } from '../../lib/ipc';
import type { ModelProfile } from '../../lib/ipc-types';

const KINDS: Exclusion['kind'][] = ['app', 'folder', 'title_pattern'];

export function ObservationSection() {
  const ob = useApp((s) => s.observation);
  const [profiles, setProfiles] = useState<ModelProfile[]>([]);
  const [kind, setKind] = useState<Exclusion['kind']>('app');
  const [pattern, setPattern] = useState('');

  useEffect(() => {
    ipc
      .invoke('list_model_profiles', {})
      .then((r) => setProfiles(r.profiles))
      .catch(() => setProfiles([]));
  }, []);

  if (!ob) return <p className="subtle">{t.common.loading}</p>;
  const { policy, status } = ob;
  const save = (patch: Partial<ObservationPolicy>) => actions.setPolicy({ ...policy, ...patch });
  const paused = status.paused;

  const stateLabel = !policy.enabled ? t.observation.status.off : paused ? t.observation.status.paused : t.observation.status.active;

  const pauseFor = (ms: number) => save({ paused_until: new Date(Date.now() + ms).toISOString() });

  return (
    <div className="sec-stack">
      <section className="sec">
        <h2>{t.observation.heading}</h2>
        <p className="sec__intro">{t.observation.intro}</p>

        <div className="setting">
          <div className="setting__text">
            <span className="setting__title">{t.observation.master}</span>
            <span className="field__help">{t.observation.masterHelp}</span>
          </div>
          <Toggle
            checked={policy.enabled}
            label={t.observation.master}
            onChange={(v) => save({ enabled: v, paused_until: v ? policy.paused_until : null })}
          />
        </div>

        <div className="statusline" role="status">
          <span className={`badge ${!policy.enabled ? '' : paused ? 'badge--warn' : 'badge--success'}`}>
            <Icon name={!policy.enabled ? 'slash' : paused ? 'pause' : 'check'} />
            {stateLabel}
          </span>
          {policy.enabled && status.last_sample_at && !paused && (
            <span className="subtle">{t.observation.lastSample(formatRelative(status.last_sample_at))}</span>
          )}
          {paused && policy.paused_until && (
            <span className="subtle">{t.observation.pausedUntil(formatWhen(policy.paused_until))}</span>
          )}
        </div>

        {policy.enabled && (
          <div className="btnrow">
            {paused ? (
              <button className="btn btn--small" onClick={() => save({ paused_until: null })}>
                {t.observation.resume}
              </button>
            ) : (
              <>
                <button className="btn btn--small" onClick={() => pauseFor(3600_000)}>
                  <Icon name="pause" />
                  {t.observation.pauseHour}
                </button>
                <button className="btn btn--small" onClick={() => pauseFor(18 * 3600_000)}>
                  {t.observation.pauseDay}
                </button>
              </>
            )}
          </div>
        )}

        <div className="setting">
          <div className="setting__text">
            <span className="setting__title">{t.observation.dnd}</span>
            <span className="field__help">{t.observation.dndHelp}</span>
          </div>
          <Toggle checked={policy.dnd} label={t.observation.dnd} onChange={(v) => save({ dnd: v })} />
        </div>
      </section>

      <section className="sec">
        <h2>{t.observation.sources}</h2>
        <ul className="settings-list">
          {policy.sources.map((src) => (
            <li key={src.id} className="setting">
              <div className="setting__text">
                <span className="setting__title">{t.observation.sourceName[src.id as ObservationSourceId]}</span>
                <span className="field__help">{t.observation.sourceHelp[src.id as ObservationSourceId]}</span>
              </div>
              <Toggle
                checked={src.enabled}
                disabled={!policy.enabled}
                label={t.observation.sourceName[src.id as ObservationSourceId]}
                onChange={(v) =>
                  save({ sources: policy.sources.map((x) => (x.id === src.id ? { ...x, enabled: v } : x)) })
                }
              />
            </li>
          ))}
        </ul>
        <p className="notice-line">
          <Icon name="shield" />
          {t.observation.notCollected}
        </p>
      </section>

      <section className="sec">
        <h2>{t.observation.exclusions}</h2>
        <p className="sec__intro">{t.observation.exclusionsHelp}</p>
        {policy.exclusions.length === 0 ? (
          <p className="subtle">{t.observation.exclusionEmpty}</p>
        ) : (
          <ul className="settings-list">
            {policy.exclusions.map((ex) => (
              <li key={ex.id} className="setting">
                <div className="setting__text">
                  <span className="subtle">{t.observation.exclusionKind[ex.kind]}</span>
                  <span className="mono">{ex.pattern}</span>
                </div>
                <button
                  className="btn btn--quiet btn--icon btn--small"
                  aria-label={`${t.common.remove}：${ex.pattern}`}
                  onClick={() => save({ exclusions: policy.exclusions.filter((x) => x.id !== ex.id) })}
                >
                  <Icon name="trash" />
                </button>
              </li>
            ))}
          </ul>
        )}
        <form
          className="addrow"
          onSubmit={(e) => {
            e.preventDefault();
            const p = pattern.trim();
            if (!p) return;
            void save({ exclusions: [...policy.exclusions, { id: `ex-${Date.now()}`, kind, pattern: p }] });
            setPattern('');
          }}
        >
          <select className="select addrow__kind" value={kind} onChange={(e) => setKind(e.target.value as Exclusion['kind'])} aria-label={t.observation.exclusions}>
            {KINDS.map((k) => (
              <option key={k} value={k}>
                {t.observation.exclusionKind[k]}
              </option>
            ))}
          </select>
          <input
            className="input mono"
            value={pattern}
            placeholder={t.observation.exclusionPlaceholder[kind]}
            aria-label={t.observation.exclusionPlaceholder[kind]}
            onChange={(e) => setPattern(e.target.value)}
          />
          <button type="submit" className="btn" disabled={!pattern.trim()}>
            <Icon name="plus" />
            {t.observation.exclusionAdd}
          </button>
        </form>
      </section>

      <section className="sec">
        <h2>{t.observation.cloud}</h2>
        <p className="sec__intro">{t.observation.cloudIntro}</p>
        <ul className="settings-list">
          <li className="setting">
            <span className="setting__title">{t.observation.cloudSummary}</span>
            <Toggle
              checked={policy.cloud_scope.send_daily_summary}
              label={t.observation.cloudSummary}
              onChange={(v) => save({ cloud_scope: { ...policy.cloud_scope, send_daily_summary: v } })}
            />
          </li>
          <li className="setting">
            <span className="setting__title">{t.observation.cloudTitles}</span>
            <Toggle
              checked={policy.cloud_scope.send_window_titles}
              label={t.observation.cloudTitles}
              onChange={(v) => save({ cloud_scope: { ...policy.cloud_scope, send_window_titles: v } })}
            />
          </li>
        </ul>
        <fieldset className="checks">
          <legend className="setting__title">{t.observation.cloudProviders}</legend>
          <p className="field__help">{t.observation.cloudProvidersHelp}</p>
          {profiles.map((p) => {
            const on = policy.cloud_scope.allowed_profile_ids.includes(p.id);
            return (
              <label key={p.id} className="check">
                <input
                  type="checkbox"
                  checked={on}
                  onChange={(e) =>
                    save({
                      cloud_scope: {
                        ...policy.cloud_scope,
                        allowed_profile_ids: e.target.checked
                          ? [...policy.cloud_scope.allowed_profile_ids, p.id]
                          : policy.cloud_scope.allowed_profile_ids.filter((x) => x !== p.id),
                      },
                    })
                  }
                />
                <span>
                  {p.display_name}
                  <span className="subtle">　{p.provider}</span>
                </span>
              </label>
            );
          })}
        </fieldset>
      </section>
    </div>
  );
}
