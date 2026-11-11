import type { TaskStatus } from '../lib/ipc-types';
import { t } from '../lib/strings';
import { Icon, type IconName } from './Icon';

const MAP: Record<TaskStatus, { icon: IconName; tone: string; spin?: boolean }> = {
  queued: { icon: 'clock', tone: '' },
  running: { icon: 'spin', tone: 'primary', spin: true },
  awaiting_user: { icon: 'alert', tone: 'attn' },
  paused: { icon: 'pause', tone: 'warn' },
  verifying: { icon: 'ring', tone: 'primary' },
  uncertain: { icon: 'question', tone: 'warn' },
  completed: { icon: 'check', tone: 'success' },
  failed: { icon: 'x', tone: 'danger' },
  cancelled: { icon: 'slash', tone: '' },
};

export function StatusBadge({ status }: { status: TaskStatus }) {
  const m = MAP[status];
  return (
    <span className={`badge ${m.tone ? `badge--${m.tone}` : ''}`}>
      <Icon name={m.icon} className={m.spin ? 'spin' : ''} />
      {t.status[status]}
    </span>
  );
}
