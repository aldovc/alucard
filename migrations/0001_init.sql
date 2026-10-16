CREATE TABLE repositories (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    name TEXT NOT NULL,
    cache_path TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE runs (
    id TEXT PRIMARY KEY,
    repository_id TEXT NOT NULL REFERENCES repositories(id),
    command TEXT NOT NULL CHECK (command IN ('run', 'continue', 'merge')),
    status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'interrupted', 'failed')),
    started_at TEXT NOT NULL,
    heartbeat_at TEXT NOT NULL,
    ended_at TEXT,
    base_sha TEXT,
    image TEXT,
    logs_path TEXT,
    pid INTEGER,
    hostname TEXT,
    reconciliation_reason TEXT,
    branch TEXT,
    retry_owner_run_id TEXT REFERENCES runs(id)
);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    repository_id TEXT NOT NULL REFERENCES repositories(id),
    source TEXT NOT NULL CHECK (source IN ('github', 'local')),
    source_path TEXT NOT NULL DEFAULT '',
    external_id TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'claimed', 'in_flight', 'done', 'human')),
    blockers_json TEXT NOT NULL,
    pr_number INTEGER,
    previous_pr_number INTEGER,
    updated_at TEXT NOT NULL,
    UNIQUE (repository_id, source, source_path, external_id)
);

CREATE TABLE claims (
    id TEXT PRIMARY KEY,
    repository_id TEXT NOT NULL REFERENCES repositories(id),
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('task', 'pr')),
    resource_key TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id),
    state TEXT NOT NULL CHECK (state IN ('active', 'released')),
    acquired_at TEXT NOT NULL,
    heartbeat_at TEXT NOT NULL
);

CREATE UNIQUE INDEX active_claim_per_resource
    ON claims (repository_id, resource_kind, resource_key)
    WHERE state = 'active';

CREATE TABLE pull_requests (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    repository_id TEXT NOT NULL REFERENCES repositories(id),
    number INTEGER NOT NULL,
    branch TEXT NOT NULL,
    head_sha TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('normal', 'recovery')),
    remote_state TEXT NOT NULL,
    gate_state TEXT NOT NULL,
    reviewer_approved_sha TEXT,
    review_verdict TEXT,
    UNIQUE (repository_id, number)
);

CREATE TABLE recovery (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    original_run_id TEXT NOT NULL REFERENCES runs(id),
    branch TEXT NOT NULL,
    pull_request_number INTEGER,
    reason TEXT NOT NULL,
    marker_posted_at TEXT,
    unparked_at TEXT,
    created_at TEXT NOT NULL,
    checkout_path TEXT,
    failure_reason TEXT
);

CREATE TABLE operations (
    id TEXT PRIMARY KEY,
    repository_id TEXT NOT NULL REFERENCES repositories(id),
    target TEXT NOT NULL,
    operation_type TEXT NOT NULL,
    phase TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    payload TEXT
);

CREATE UNIQUE INDEX recovery_per_run ON recovery (original_run_id);
CREATE UNIQUE INDEX operation_target_type ON operations (repository_id, target, operation_type);

CREATE TABLE measurement_summaries (
    run_id TEXT NOT NULL REFERENCES runs(id),
    iteration INTEGER NOT NULL,
    measurements_path TEXT NOT NULL,
    total_invocations INTEGER NOT NULL,
    costed_invocations INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (run_id, iteration)
);
