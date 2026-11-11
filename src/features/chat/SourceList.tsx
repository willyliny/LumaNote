import type { Source } from '../../lib/ipc-types';
import { t } from '../../lib/strings';
import { openExternal } from '../../lib/external';
import { Icon } from '../../components/Icon';

export function SourceList({ sources }: { sources: Source[] }) {
  if (sources.length === 0) return null;
  return (
    <details className="sources">
      <summary>
        <Icon name="chevron" className="sources__chev" />
        {t.chat.sources(sources.length)}
      </summary>
      <ul>
        {sources.map((s) => (
          <li key={s.id} className="source">
            <Icon name={s.url ? 'link' : 'file'} />
            <div className="source__text">
              {s.url ? (
                <button className="source__link" onClick={() => openExternal(s.url!)}>
                  {s.title}
                </button>
              ) : (
                <span className="source__title">{s.title}</span>
              )}
              {s.path && <span className="mono">{s.path}</span>}
              {s.quote && (
                <q className="source__quote">
                  {t.chat.quote}：{s.quote}
                </q>
              )}
            </div>
          </li>
        ))}
      </ul>
    </details>
  );
}
