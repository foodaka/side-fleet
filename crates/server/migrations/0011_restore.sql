-- 'agent' runs the coding agent; 'restore' puts the thread's worktree back to a
-- snapshot (restore_to, a commit under refs/sidefleet/snapshots/ on the worker).
ALTER TABLE runs ADD COLUMN kind text NOT NULL DEFAULT 'agent';
ALTER TABLE runs ADD COLUMN restore_to text;
