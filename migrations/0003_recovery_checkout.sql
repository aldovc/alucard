ALTER TABLE recovery ADD COLUMN checkout_path TEXT;
ALTER TABLE recovery ADD COLUMN failure_reason TEXT;

CREATE UNIQUE INDEX recovery_per_run ON recovery (original_run_id);
CREATE UNIQUE INDEX operation_target_type ON operations (repository_id, target, operation_type);
