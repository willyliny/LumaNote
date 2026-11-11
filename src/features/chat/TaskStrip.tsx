import type { Task } from '../../lib/ipc-types';
import { t } from '../../lib/strings';
import { StatusBadge } from '../../components/StatusBadge';
import { ApprovalCard } from '../../components/ApprovalCard';
import { actions } from '../../state/app-store';
import { ArtifactList } from './ArtifactList';

/** Compact task state shown inline in the conversation under the task's last message. */
export function TaskStrip({ task, onOpenTasks }: { task: Task; onOpenTasks: () => void }) {
  const lastStep = task.steps[task.steps.length - 1];
  const attention = task.status === 'failed' || task.status === 'uncertain' || task.status === 'paused';
  return (
    <div className="strip" data-task={task.id}>
      <div className="strip__row">
        <StatusBadge status={task.status} />
        {lastStep && task.status !== 'completed' && (
          <span className="strip__step">{lastStep.title}</span>
        )}
        <button className="btn btn--quiet btn--small strip__link" onClick={onOpenTasks}>
          {t.chat.taskLink}
        </button>
      </div>
      {attention && (task.error || task.needs_user) && (
        <p className="strip__note">{task.needs_user?.summary ?? t.errors.hint[task.error!.kind]}</p>
      )}
      {task.pending_approval && (
        <ApprovalCard
          approval={task.pending_approval}
          onRespond={(d) => actions.respondApproval(task.pending_approval!.approval_id, d)}
        />
      )}
      <ArtifactList artifacts={task.artifacts} />
    </div>
  );
}
