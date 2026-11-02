-- When a stop was first asked for, so a run whose worker never acts on it can be
-- stopped here. Runs already waiting on a stop count from now.
ALTER TABLE runs ADD COLUMN stop_requested_at timestamptz;
UPDATE runs SET stop_requested_at = now() WHERE status = 'running' AND stop_requested;
