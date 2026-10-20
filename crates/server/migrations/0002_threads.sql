-- Follow-up messages: a run can continue an earlier run's Claude Code session.
ALTER TABLE runs ADD COLUMN parent_id uuid REFERENCES runs(id) ON DELETE CASCADE;
ALTER TABLE runs ADD COLUMN session_id text;
ALTER TABLE runs ADD COLUMN resume_session_id text;
ALTER TABLE runs ADD COLUMN pinned_worker_id text;
ALTER TABLE runs ADD COLUMN permission_mode text;
CREATE INDEX runs_parent_idx ON runs (parent_id, created_at);
