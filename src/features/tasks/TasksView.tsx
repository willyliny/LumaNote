import { t } from '../../lib/strings';
import { GROUP_ORDER, groupOf, selectJobs, useApp, type TaskGroup } from '../../state/app-store';
import type { Task } from '../../lib/ipc-types';
import { TaskCard } from './TaskCard';
import './tasks.css';

export function TasksView() {
  const tasks = useApp((s) => s.tasks);
  const jobs = selectJobs(tasks);
  const groups: Record<TaskGroup, Task[]> = { waiting: [], running: [], pending: [], failed: [], done: [] };
  for (const j of jobs) groups[groupOf(j.status)].push(j);
  for (const g of GROUP_ORDER) groups[g].sort((a, b) => b.updated_at.localeCompare(a.updated_at));

  if (jobs.length === 0) {
    return (
      <div className="tasks tasks--empty">
        <p>{t.tasks.empty}</p>
      </div>
    );
  }
  return (
    <div className="tasks">
      {GROUP_ORDER.filter((g) => groups[g].length > 0).map((g) => (
        <section key={g} className="group" aria-labelledby={`g-${g}`}>
          <h2 id={`g-${g}`} className={`group__title group__title--${g}`}>
            {t.tasks.groups[g]}
            <span className="group__count">{groups[g].length}</span>
          </h2>
          <div className="group__list">
            {groups[g].map((task) => (
              <TaskCard key={task.id} task={task} />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
