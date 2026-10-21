-- Failover (opt-in per run): when a run's machine drops, queue a replacement on
-- another machine that continues from the run's last pushed checkpoint.
ALTER TABLE runs ADD COLUMN failover boolean NOT NULL DEFAULT false;
-- On the replacement run: the lost run it continues (a moved run does not move again).
ALTER TABLE runs ADD COLUMN moved_from uuid REFERENCES runs(id) ON DELETE SET NULL;
-- On the replacement run: the pushed branch to create its worktree from.
ALTER TABLE runs ADD COLUMN resume_from_branch text;
-- Machines that must not claim this run (the one that lost it).
ALTER TABLE runs ADD COLUMN excluded_worker_ids text[] NOT NULL DEFAULT '{}';
