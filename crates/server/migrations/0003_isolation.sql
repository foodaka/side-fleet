-- How a run works on the repo: 'worktree' (own checkout + branch, the default) or 'checkout' (in place).
ALTER TABLE runs ADD COLUMN isolation text NOT NULL DEFAULT 'worktree';
