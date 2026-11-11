import { useState } from 'react';
import type { ApprovalRequest } from '../lib/ipc-types';
import { formatWhen } from '../lib/format';
import { t } from '../lib/strings';
import { Icon } from './Icon';
import './approval.css';

const MARK = { add: '+', remove: '−', context: ' ' } as const;
const MARK_LABEL = { add: t.approval.added, remove: t.approval.removed, context: '' } as const;

/**
 * Handbook 8: shows action, target, preview/diff, consequence; approve once or deny.
 * Approving applies to this exact operation only; the card says so.
 */
export function ApprovalCard({
  approval,
  onRespond,
}: {
  approval: ApprovalRequest;
  onRespond: (decision: 'approve_once' | 'deny') => Promise<unknown> | void;
}) {
  const [busy, setBusy] = useState(false);
  const respond = async (d: 'approve_once' | 'deny') => {
    setBusy(true);
    try {
      await onRespond(d);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="approval" aria-labelledby={`ap-${approval.approval_id}`}>
      <header className="approval__head">
        <Icon name="alert" />
        <h3 id={`ap-${approval.approval_id}`}>{t.approval.heading}</h3>
        {approval.expires_at && (
          <span className="approval__expiry">{t.approval.expires(formatWhen(approval.expires_at))}</span>
        )}
      </header>

      <dl className="approval__facts">
        <div>
          <dt>{t.approval.action}</dt>
          <dd>{t.approval.actions[approval.action]}</dd>
        </div>
        <div>
          <dt>{t.approval.target}</dt>
          <dd className="mono">{approval.target}</dd>
        </div>
      </dl>

      <div className="approval__preview">
        <p className="approval__label">
          {approval.preview.kind === 'diff' ? t.approval.diff : t.approval.preview}
        </p>
        {approval.preview.kind === 'diff' ? (
          <pre className="diff" tabIndex={0} aria-label={t.approval.diff}>
            {approval.preview.lines.map((l, i) => (
              <span key={i} className={`diff__line diff__line--${l.op}`}>
                <span className="diff__mark" aria-label={MARK_LABEL[l.op] || undefined}>
                  {MARK[l.op]}
                </span>
                <span className="diff__text">{l.text || ' '}</span>
              </span>
            ))}
          </pre>
        ) : (
          <pre className="diff diff--plain" tabIndex={0}>
            {approval.preview.text}
          </pre>
        )}
      </div>

      <div className="approval__consequence">
        <p className="approval__label">{t.approval.consequence}</p>
        <ul>
          {approval.consequences.map((c) => (
            <li key={c}>{c}</li>
          ))}
        </ul>
      </div>

      <p className="approval__note">
        {approval.once_only_reason === 'external_content' ? t.approval.onceExternal : t.approval.onceNote}
      </p>

      <div className="approval__actions">
        <button className="btn btn--primary" disabled={busy} onClick={() => respond('approve_once')}>
          <Icon name="check" />
          {t.approval.approveOnce}
        </button>
        <button className="btn btn--danger" disabled={busy} onClick={() => respond('deny')}>
          <Icon name="x" />
          {t.approval.deny}
        </button>
      </div>
    </section>
  );
}
