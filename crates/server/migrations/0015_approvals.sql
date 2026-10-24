-- "Ask me": tool calls a running agent wants to make, waiting for someone on the
-- page to approve or deny them. Only a pending approval changes, and only once
-- (see `ApprovalStatus::after`).
CREATE TABLE approvals (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    run_id      uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    tool_name   text NOT NULL,
    input       jsonb NOT NULL,
    tool_use_id text,
    status      text NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'approved', 'denied', 'expired', 'cancelled')),
    message     text,
    decided_by  text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    decided_at  timestamptz
);
CREATE INDEX approvals_run_idx ON approvals (run_id, created_at);
CREATE INDEX approvals_pending_idx ON approvals (run_id) WHERE status = 'pending';
