-- 'pr' runs commit and push a thread's branch and open a pull request. The
-- root run keeps the pull request (url, number, state), the worker that opened
-- it (and checks on it, with its own GitHub sign-in), and when it last did.
ALTER TABLE runs ADD COLUMN pr jsonb;
ALTER TABLE runs ADD COLUMN pr_worker text;
ALTER TABLE runs ADD COLUMN pr_checked_at timestamptz;
CREATE INDEX runs_open_prs ON runs (org_id, pr_worker) WHERE pr->>'state' = 'open';
