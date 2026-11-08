ALTER TABLE workers ADD COLUMN clean_worktrees_requested boolean NOT NULL DEFAULT false;
ALTER TABLE workers ADD COLUMN worktree_cleanup jsonb;
