-- 0002: task execution columns (handbook 7.5 / 9, v0.3 amendments), local todos,
-- approvals and step results used by the orchestrator.

-- tasks: columns the handbook lists but 0001 lacked, plus two orchestrator helpers
-- (conversation_id: where the task's messages live; tool_call_base: tool calls spent
-- before the current budget window, so a resume grants a fresh window).
ALTER TABLE tasks ADD COLUMN parent_task_id TEXT REFERENCES tasks(id);
ALTER TABLE tasks ADD COLUMN limits_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE tasks ADD COLUMN tool_call_count INTEGER NOT NULL DEFAULT 0 CHECK (tool_call_count >= 0);
ALTER TABLE tasks ADD COLUMN started_at TEXT;
ALTER TABLE tasks ADD COLUMN conversation_id TEXT REFERENCES conversations(id);
ALTER TABLE tasks ADD COLUMN tool_call_base INTEGER NOT NULL DEFAULT 0 CHECK (tool_call_base >= 0);
CREATE INDEX idx_tasks_parent ON tasks(parent_task_id);

-- task_steps: the saved tool result, so a resume never has to re-run a finished step.
ALTER TABLE task_steps ADD COLUMN result_json TEXT;

-- Local todos created by the create_todos tool. A date the model was unsure about is
-- stored with due_uncertain = 1 (due_at may then be NULL or a tentative value, and
-- due_text keeps the user's original wording).
CREATE TABLE todos (
    id            TEXT PRIMARY KEY,
    task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    title         TEXT NOT NULL CHECK (length(trim(title)) > 0),
    due_at        TEXT,
    due_uncertain INTEGER NOT NULL DEFAULT 0 CHECK (due_uncertain IN (0, 1)),
    due_text      TEXT,
    status        TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','done','cancelled')),
    source_ref    TEXT,
    created_at    TEXT NOT NULL
);
CREATE INDEX idx_todos_task ON todos(task_id);
CREATE INDEX idx_todos_due ON todos(due_at);

-- Pending / resolved user decisions for tool calls that need approval.
CREATE TABLE approvals (
    id             TEXT PRIMARY KEY,
    task_id        TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tool_call_id   TEXT NOT NULL,
    tool           TEXT NOT NULL,
    action         TEXT NOT NULL,
    resource       TEXT NOT NULL,
    arguments_json TEXT NOT NULL,
    arguments_hash TEXT NOT NULL,
    preview        TEXT NOT NULL,
    reason         TEXT NOT NULL,
    status         TEXT NOT NULL CHECK (status IN ('pending','approved','denied','expired')),
    created_at     TEXT NOT NULL,
    responded_at   TEXT,
    grant_id       TEXT REFERENCES grants(id),
    UNIQUE (task_id, tool_call_id)
);
CREATE INDEX idx_approvals_status ON approvals(status);
