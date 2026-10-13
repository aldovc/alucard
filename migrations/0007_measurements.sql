CREATE TABLE measurement_summaries (
    run_id TEXT NOT NULL REFERENCES runs(id),
    iteration INTEGER NOT NULL,
    measurements_path TEXT NOT NULL,
    total_invocations INTEGER NOT NULL,
    costed_invocations INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (run_id, iteration)
);
