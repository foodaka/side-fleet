-- Reasoning effort a run asked for (Claude Code's --effort levels, ultrathink,
-- ultracode; Codex's model_reasoning_effort). NULL means the agent's default.
ALTER TABLE runs ADD COLUMN effort text;
