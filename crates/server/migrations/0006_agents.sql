-- Which coding agent runs a task: 'claude' (Claude Code), 'codex' or 'opencode'.
-- Existing runs were all Claude Code. A thread keeps the agent of its first run.
ALTER TABLE runs ADD COLUMN agent text NOT NULL DEFAULT 'claude';
