-- 0001: initial schema (handbook section 9, v0.3). Timestamps are RFC 3339 UTC text.

CREATE TABLE conversations (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    archived_at TEXT
);

CREATE TABLE tasks (
    id                TEXT PRIMARY KEY,
    intent            TEXT NOT NULL,
    status            TEXT NOT NULL CHECK (status IN
        ('queued','running','awaiting_user','paused','verifying','uncertain','completed','failed','cancelled')),
    scope_json        TEXT NOT NULL DEFAULT '{}',
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    result_ref        TEXT,
    untrusted_content INTEGER NOT NULL DEFAULT 0 CHECK (untrusted_content IN (0, 1))
);
CREATE INDEX idx_tasks_status ON tasks(status);

CREATE TABLE task_steps (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    tool            TEXT NOT NULL,
    args_redacted   TEXT,
    side_effect     TEXT NOT NULL CHECK (side_effect IN ('none','local_write','external_write')),
    status          TEXT NOT NULL CHECK (status IN ('started','succeeded','failed','uncertain','interrupted')),
    idempotency_key TEXT,
    started_at      TEXT NOT NULL,
    finished_at     TEXT,
    evidence_ref    TEXT,
    UNIQUE (task_id, seq),
    UNIQUE (task_id, idempotency_key)
);
CREATE INDEX idx_task_steps_status ON task_steps(status);

CREATE TABLE task_events (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    sequence     INTEGER NOT NULL,
    type         TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at   TEXT NOT NULL,
    UNIQUE (task_id, sequence)
);

CREATE TABLE messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    task_id         TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    role            TEXT NOT NULL CHECK (role IN ('system','user','assistant','tool')),
    content_json    TEXT NOT NULL,
    tool_call_id    TEXT,
    created_at      TEXT NOT NULL
);
CREATE INDEX idx_messages_conversation ON messages(conversation_id);

CREATE TABLE artifacts (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    path         TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    bytes        INTEGER NOT NULL CHECK (bytes >= 0),
    created_at   TEXT NOT NULL
);
CREATE INDEX idx_artifacts_task ON artifacts(task_id);

CREATE TABLE consents (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    version     INTEGER NOT NULL,
    granted_at  TEXT NOT NULL,
    revoked_at  TEXT,
    detail_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE activity_segments (
    id              TEXT PRIMARY KEY,
    app_id          TEXT NOT NULL,
    start_at        TEXT NOT NULL,
    end_at          TEXT NOT NULL,
    active_ms       INTEGER NOT NULL CHECK (active_ms >= 0),
    source          TEXT NOT NULL,
    consent_version INTEGER NOT NULL
);
CREATE INDEX idx_activity_start ON activity_segments(start_at);

CREATE TABLE summaries (
    id             TEXT PRIMARY KEY,
    period_start   TEXT NOT NULL,
    period_end     TEXT NOT NULL,
    body           TEXT NOT NULL,
    source_ids     TEXT NOT NULL DEFAULT '[]',
    version        INTEGER NOT NULL DEFAULT 1,
    invalidated_at TEXT
);

CREATE TABLE memories (
    id             TEXT PRIMARY KEY,
    type           TEXT NOT NULL CHECK (type IN
        ('explicit_preference','task_fact','short_term_context','inferred_habit')),
    content        TEXT NOT NULL,
    confidence     REAL,
    source_ids     TEXT NOT NULL DEFAULT '[]',
    confirmed_at   TEXT,
    expires_at     TEXT,
    invalidated_at TEXT
);

CREATE TABLE suggestions (
    id             TEXT PRIMARY KEY,
    rule_id        TEXT NOT NULL,
    evidence_refs  TEXT NOT NULL DEFAULT '[]',
    shown_at       TEXT,
    outcome        TEXT,
    cooldown_until TEXT
);

CREATE TABLE schedules (
    id            TEXT PRIMARY KEY,
    task_template TEXT NOT NULL,
    timezone      TEXT NOT NULL,
    next_run_at   TEXT,
    recurrence    TEXT,
    misfire_policy TEXT NOT NULL DEFAULT 'coalesce',
    enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
);

CREATE TABLE deliveries (
    id             TEXT PRIMARY KEY,
    schedule_id    TEXT NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
    occurrence_key TEXT NOT NULL,
    status         TEXT NOT NULL,
    delivered_at   TEXT,
    UNIQUE (schedule_id, occurrence_key)
);

CREATE TABLE grants (
    id             TEXT PRIMARY KEY,
    action         TEXT NOT NULL,
    resource_scope TEXT NOT NULL,
    arguments_hash TEXT,
    expires_at     TEXT,
    max_uses       INTEGER CHECK (max_uses IS NULL OR max_uses >= 0),
    used_count     INTEGER NOT NULL DEFAULT 0 CHECK (used_count >= 0),
    source_task    TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    revoked_at     TEXT,
    created_at     TEXT NOT NULL
);
CREATE INDEX idx_grants_action ON grants(action);

CREATE TABLE connections (
    id             TEXT PRIMARY KEY,
    provider       TEXT NOT NULL,
    granted_scopes TEXT NOT NULL DEFAULT '[]',
    credential_ref TEXT,
    status         TEXT NOT NULL
);

CREATE TABLE model_profiles (
    id                TEXT PRIMARY KEY,
    provider          TEXT NOT NULL,
    protocol          TEXT NOT NULL,
    base_url          TEXT NOT NULL,
    model_id          TEXT NOT NULL,
    credential_ref    TEXT,
    capabilities_json TEXT NOT NULL DEFAULT '{}',
    verified_at       TEXT,
    config_version    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE model_routes (
    id                  TEXT PRIMARY KEY,
    purpose             TEXT NOT NULL UNIQUE,
    primary_profile_id  TEXT NOT NULL REFERENCES model_profiles(id),
    fallback_profile_ids TEXT NOT NULL DEFAULT '[]',
    allowed_data_classes TEXT NOT NULL DEFAULT '[]',
    budget_policy       TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE model_runs (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    profile_id          TEXT NOT NULL REFERENCES model_profiles(id),
    config_version      INTEGER NOT NULL,
    provider_request_id TEXT,
    status              TEXT NOT NULL,
    usage_json          TEXT,
    estimated_cost      REAL,
    latency_ms          INTEGER
);

CREATE TABLE exclusions (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    pattern    TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
