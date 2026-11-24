-- Run tonight: a night task waits for its start time, is only taken by a machine with nothing
-- else running, and is stopped at stop_at if it is still going. When the night is over, one
-- push sums it up (night_reported_at), instead of one per task in the small hours.
ALTER TABLE runs ADD COLUMN stop_at timestamptz;
ALTER TABLE runs ADD COLUMN night_reported_at timestamptz;
CREATE INDEX runs_night_open ON runs (stop_at) WHERE stop_at IS NOT NULL AND night_reported_at IS NULL;
