-- 0003: explicit grant scope, step-level grant consumption record, schedule soft delete.

-- grants.scope: 'task' grants apply only to requests from grants.source_task;
-- 'user_standing' grants (created by an explicit user action in settings) apply to every
-- task. Existing rows: anything with a source_task becomes task-scoped. Rows without a
-- source_task already applied to every task before this migration, so they stay
-- user-standing (no widening and no silent loss of what the user set up).
-- New rows default to the narrow value.
ALTER TABLE grants ADD COLUMN scope TEXT NOT NULL DEFAULT 'task'
    CHECK (scope IN ('task', 'user_standing'));
UPDATE grants SET scope = CASE WHEN source_task IS NULL THEN 'user_standing' ELSE 'task' END;

-- Task-scoped grants of tasks that are already finished must not stay live (a crash
-- between the terminal transition and the revocation could have left them behind).
UPDATE grants
   SET revoked_at = (SELECT t.updated_at FROM tasks t WHERE t.id = grants.source_task)
 WHERE scope = 'task'
   AND revoked_at IS NULL
   AND source_task IN (SELECT id FROM tasks WHERE status IN ('completed', 'failed', 'cancelled'));

-- Which grant use was consumed for a step, written in the same transaction as the step
-- itself, so recovery can tell what an interrupted step spent.
ALTER TABLE task_steps ADD COLUMN consumed_grant_id TEXT REFERENCES grants(id) ON DELETE SET NULL;

-- Soft delete: the schedule disappears from listings and the engine, its deliveries
-- (trigger history) stay, and UNIQUE (schedule_id, occurrence_key) keeps working.
ALTER TABLE schedules ADD COLUMN deleted_at TEXT;
CREATE INDEX idx_schedules_live ON schedules(next_run_at) WHERE deleted_at IS NULL;
