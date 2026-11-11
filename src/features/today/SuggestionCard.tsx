import { useState } from 'react';
import type { Suggestion } from '../../lib/ipc-types';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';
import { actions } from '../../state/app-store';

/** Handbook 6: why it appeared, data used, do it / later / don't suggest again. */
export function SuggestionCard({ s }: { s: Suggestion }) {
  const [busy, setBusy] = useState(false);
  const respond = async (r: 'accept' | 'later' | 'never') => {
    setBusy(true);
    await actions.respondSuggestion(s.id, r);
    setBusy(false);
  };
  return (
    <article className="suggest" aria-label={s.title}>
      <header className="suggest__head">
        <Icon name="leaf" />
        <h3>{s.title}</h3>
      </header>
      <p className="suggest__body">{s.body}</p>
      <dl className="suggest__why">
        <div>
          <dt>{t.today.why}</dt>
          <dd>{s.why}</dd>
        </div>
        <div>
          <dt>{t.today.dataUsed}</dt>
          <dd>
            <ul>
              {s.data_used.map((d) => (
                <li key={d}>{d}</li>
              ))}
            </ul>
          </dd>
        </div>
      </dl>
      <div className="suggest__actions">
        <button className="btn btn--primary btn--small" disabled={busy} onClick={() => respond('accept')}>
          {t.today.doIt}
        </button>
        <button className="btn btn--small" disabled={busy} onClick={() => respond('later')}>
          {t.today.later}
        </button>
        <button className="btn btn--quiet btn--small" disabled={busy} onClick={() => respond('never')}>
          {t.today.never}
        </button>
      </div>
    </article>
  );
}
