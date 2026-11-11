import { useEffect } from 'react';
import { appStore, dismissNotice, useApp } from '../state/app-store';
import { t } from '../lib/strings';
import { Icon } from './Icon';

export function Toast() {
  const notice = useApp((s) => s.notice);
  useEffect(() => {
    if (!notice) return;
    const id = notice.id;
    const timer = setTimeout(() => {
      if (appStore.get().notice?.id === id) dismissNotice();
    }, notice.tone === 'error' ? 9000 : 5000);
    return () => clearTimeout(timer);
  }, [notice]);
  if (!notice) return null;
  return (
    <div
      className={`toast ${notice.tone === 'error' ? 'toast--error' : ''}`}
      role={notice.tone === 'error' ? 'alert' : 'status'}
    >
      <div className="toast__body">
        <span className="toast__title">{notice.text}</span>
        {notice.detail && <span className="toast__detail">{notice.detail}</span>}
      </div>
      <button className="btn btn--quiet btn--icon btn--small" onClick={dismissNotice} aria-label={t.common.close}>
        <Icon name="x" />
      </button>
    </div>
  );
}
