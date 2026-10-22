-- A fork is a new thread that continues from a message of another one, with any
-- agent and model: forked_from is that message (run), fork_commit the snapshot
-- its worktree starts from, and context the transcript handed to the new agent.
ALTER TABLE runs ADD COLUMN forked_from uuid REFERENCES runs(id) ON DELETE SET NULL;
ALTER TABLE runs ADD COLUMN fork_commit text;
ALTER TABLE runs ADD COLUMN context text;
