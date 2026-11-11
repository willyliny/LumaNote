-- 0004: fixes from the independent security review (handbook 8.1).
-- Every statement is additive: no existing row is changed except the documented
-- back-fills (message trust, pending approval expiry), and nothing is dropped.

-- grants.tool: a one-shot approval is bound to the tool that asked for it (not only to
-- the action, the target and the arguments hash). NULL on old rows = not tool-bound.
ALTER TABLE grants ADD COLUMN tool TEXT;

-- approvals: expiry, the resolved real path shown to the user, and the full payload
-- (capped by the application) kept only while the approval is pending.
ALTER TABLE approvals ADD COLUMN expires_at TEXT;
ALTER TABLE approvals ADD COLUMN resolved_resource TEXT;
ALTER TABLE approvals ADD COLUMN detail TEXT;
ALTER TABLE approvals ADD COLUMN detail_truncated INTEGER NOT NULL DEFAULT 0
    CHECK (detail_truncated IN (0, 1));
-- Requests that are still waiting get the default 10 minutes counted from their creation.
UPDATE approvals
   SET expires_at = strftime('%Y-%m-%dT%H:%M:%S', created_at, '+10 minutes') || substr(created_at, 20)
 WHERE status = 'pending' AND expires_at IS NULL;

-- messages.trust: how far the content of a message can be trusted. The conversation-level
-- untrusted flag is derived from this column, so the flag cannot disagree with the
-- stored context. messages.args_redacted: tool-call arguments in the stored message are
-- redacted (the complete arguments of an unanswered call live in pending_calls).
ALTER TABLE messages ADD COLUMN trust TEXT NOT NULL DEFAULT 'user_provided'
    CHECK (trust IN ('user_provided', 'local_granted', 'external'));
ALTER TABLE messages ADD COLUMN args_redacted INTEGER NOT NULL DEFAULT 0
    CHECK (args_redacted IN (0, 1));
-- Back-fill: tool results that were stored with the labels the orchestrator puts on
-- local and external content already carry that trust.
UPDATE messages SET trust = 'external'
 WHERE role = 'tool' AND content_json LIKE '%"content":"[UNTRUSTED EXTERNAL CONTENT%';
UPDATE messages SET trust = 'local_granted'
 WHERE role = 'tool' AND trust = 'user_provided'
   AND content_json LIKE '%"content":"[source: local file content%';
CREATE INDEX idx_messages_untrusted ON messages(conversation_id) WHERE trust != 'user_provided';

-- Complete arguments of tool calls that have no result yet; removed in the transaction
-- that stores the result. Messages keep only redacted arguments.
CREATE TABLE pending_calls (
    task_id        TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tool_call_id   TEXT NOT NULL,
    name           TEXT NOT NULL,
    arguments_json TEXT NOT NULL,
    -- set when the call is not to be run (over the per-turn cap, oversized arguments); it
    -- is answered with this text, in order, like any other call
    refusal        TEXT,
    created_at     TEXT NOT NULL,
    PRIMARY KEY (task_id, tool_call_id)
);

-- How every tool call of a task ended. Completion looks here for write attempts that
-- were denied or failed and have not been superseded by a successful equivalent.
CREATE TABLE call_outcomes (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tool_call_id TEXT NOT NULL,
    tool         TEXT NOT NULL,
    resource     TEXT,
    outcome      TEXT NOT NULL CHECK (outcome IN
        ('succeeded','denied','expired','failed','uncertain','rejected','not_executed')),
    is_write     INTEGER NOT NULL CHECK (is_write IN (0, 1)),
    detail       TEXT,
    created_at   TEXT NOT NULL
);
CREATE INDEX idx_call_outcomes_task ON call_outcomes(task_id, id);

-- todos: idempotency, so a re-run after a crash between "rows inserted" and "step
-- finished" does not create the same todos twice.
ALTER TABLE todos ADD COLUMN idempotency_key TEXT;
ALTER TABLE todos ADD COLUMN item_index INTEGER;
CREATE UNIQUE INDEX idx_todos_idempotency ON todos(task_id, idempotency_key, item_index)
    WHERE idempotency_key IS NOT NULL;
