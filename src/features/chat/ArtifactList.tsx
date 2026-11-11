import { useState } from 'react';
import type { Artifact } from '../../lib/ipc-types';
import { formatBytes } from '../../lib/format';
import { t } from '../../lib/strings';
import { Icon } from '../../components/Icon';

export function ArtifactList({ artifacts }: { artifacts: Artifact[] }) {
  const [copied, setCopied] = useState<string | null>(null);
  if (artifacts.length === 0) return null;
  const copy = async (a: Artifact) => {
    try {
      await navigator.clipboard.writeText(a.path);
      setCopied(a.id);
      setTimeout(() => setCopied((c) => (c === a.id ? null : c)), 1800);
    } catch {
      /* clipboard may be blocked; the path is still visible */
    }
  };
  return (
    <ul className="artifacts" aria-label={t.chat.artifacts}>
      {artifacts.map((a) => (
        <li key={a.id} className="artifact">
          <Icon name="file" />
          <div className="artifact__text">
            <span className="artifact__title">{a.title}</span>
            <span className="mono artifact__path">{a.path}</span>
          </div>
          <span className="subtle">{formatBytes(a.bytes)}</span>
          <button className="btn btn--quiet btn--small" onClick={() => copy(a)}>
            <Icon name={copied === a.id ? 'check' : 'copy'} />
            {copied === a.id ? t.common.copied : t.common.copyPath}
          </button>
        </li>
      ))}
    </ul>
  );
}
