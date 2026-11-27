-- The agent control plane: an inventory of agents, their activity, policies checked at
-- every tool call, approvals bound to the exact action, and a tamper-evident audit log.

-- Agents. Sidefleet-run ones are discovered: one per org, machine and agent kind.
-- Others register through the API and hold an agent key (only its SHA-256 is kept).
CREATE TABLE agents (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name            text NOT NULL,
    description     text NOT NULL DEFAULT '',
    owner_id        uuid REFERENCES users(id) ON DELETE SET NULL,
    team            text NOT NULL DEFAULT '',
    kind            text NOT NULL CHECK (kind IN ('claude', 'codex', 'opencode', 'custom')),
    provider        text NOT NULL DEFAULT '',
    model           text NOT NULL DEFAULT '',
    runtime         text NOT NULL DEFAULT '',
    worker_id       text,
    host            text NOT NULL DEFAULT '',
    environment     text NOT NULL DEFAULT '',
    risk            text NOT NULL DEFAULT 'unknown' CHECK (risk IN ('low', 'medium', 'high', 'unknown')),
    source          text NOT NULL CHECK (source IN ('discovered', 'registered')),
    -- What a registered agent last reported (`running` after task_started, else `idle`).
    reported_status text,
    key_hash        text,
    key_prefix      text,
    key_revoked_at  timestamptz,
    suspended_at    timestamptz,
    suspended_by    text,
    suspend_reason  text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_active_at  timestamptz
);
CREATE UNIQUE INDEX agents_discovered_idx ON agents (org_id, worker_id, kind) WHERE source = 'discovered';
CREATE UNIQUE INDEX agents_key_idx ON agents (key_hash) WHERE key_hash IS NOT NULL;
CREATE INDEX agents_org_idx ON agents (org_id, last_active_at DESC NULLS LAST);

-- Each suspension, kept after reactivation as the agent's incident history.
CREATE TABLE agent_incidents (
    id                  uuid PRIMARY KEY,
    org_id              uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    agent_id            uuid NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    suspended_at        timestamptz NOT NULL DEFAULT now(),
    suspended_by        text NOT NULL,
    reason              text NOT NULL,
    revoked             text[] NOT NULL DEFAULT '{}',
    cancelled_runs      int NOT NULL DEFAULT 0,
    cancelled_approvals int NOT NULL DEFAULT 0,
    not_stopped         text,
    reactivated_at      timestamptz,
    reactivated_by      text
);
CREATE INDEX agent_incidents_agent_idx ON agent_incidents (agent_id, suspended_at DESC);

CREATE TABLE policies (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name        text NOT NULL,
    description text NOT NULL DEFAULT '',
    scope       text NOT NULL CHECK (scope IN ('org', 'team', 'agent')),
    -- A team name for `team`, an agent id for `agent`, empty for `org`.
    applies_to  text NOT NULL DEFAULT '',
    tools       text[] NOT NULL,
    category    text NOT NULL DEFAULT '',
    decision    text NOT NULL CHECK (decision IN ('allow', 'require_approval', 'deny')),
    priority    int NOT NULL,
    active      boolean NOT NULL DEFAULT true,
    -- The seeded policy this was, e.g. `default_deny`, which cannot be deleted.
    builtin     text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    updated_by  text
);
CREATE INDEX policies_org_idx ON policies (org_id, priority);
-- Orgs get the default policies once, the first time anything reads them.
ALTER TABLE orgs ADD COLUMN policies_seeded boolean NOT NULL DEFAULT false;

-- What agents did: tasks, tool calls and their decisions, approvals, suspensions.
CREATE TABLE agent_events (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    agent_id    uuid REFERENCES agents(id) ON DELETE SET NULL,
    run_id      uuid REFERENCES runs(id) ON DELETE SET NULL,
    at          timestamptz NOT NULL DEFAULT now(),
    type        text NOT NULL,
    tool        text NOT NULL DEFAULT '',
    action      text NOT NULL DEFAULT '',
    context     jsonb NOT NULL DEFAULT '{}'::jsonb,
    outcome     text NOT NULL,
    policy_id   uuid,
    policy_name text,
    decision    text,
    metadata    jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX agent_events_org_idx ON agent_events (org_id, at DESC);
CREATE INDEX agent_events_agent_idx ON agent_events (agent_id, at DESC);

-- Approvals a policy asks for live with the "Ask me" ones. A custom agent's approval has
-- no run.
ALTER TABLE approvals ALTER COLUMN run_id DROP NOT NULL;
ALTER TABLE approvals
    ADD COLUMN kind        text NOT NULL DEFAULT 'ask' CHECK (kind IN ('ask', 'policy')),
    ADD COLUMN agent_id    uuid REFERENCES agents(id) ON DELETE SET NULL,
    ADD COLUMN policy_id   uuid,
    ADD COLUMN policy_name text,
    ADD COLUMN digest      text,
    ADD COLUMN action      text,
    ADD COLUMN context     jsonb NOT NULL DEFAULT '{}'::jsonb;
CREATE INDEX approvals_org_idx ON approvals (org_id, created_at DESC);
CREATE INDEX approvals_agent_pending_idx ON approvals (agent_id) WHERE status = 'pending';

-- The audit log. Append-only: each record carries the hash of the one before it in its
-- org, so an edit or a deletion breaks the chain. No foreign keys, so records outlive
-- what they describe.
CREATE TABLE audit_log (
    seq        bigserial PRIMARY KEY,
    id         uuid NOT NULL UNIQUE,
    org_id     uuid NOT NULL,
    at         timestamptz NOT NULL,
    actor      text NOT NULL,
    actor_kind text NOT NULL,
    event      text NOT NULL,
    agent_id   uuid,
    target     text NOT NULL DEFAULT '',
    detail     text NOT NULL DEFAULT '',
    data       jsonb NOT NULL DEFAULT '{}'::jsonb,
    prev_hash  text NOT NULL,
    hash       text NOT NULL
);
CREATE INDEX audit_log_org_idx ON audit_log (org_id, seq DESC);
CREATE INDEX audit_log_agent_idx ON audit_log (agent_id, seq DESC);

CREATE FUNCTION audit_log_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'audit_log is append-only';
END
$$;
CREATE TRIGGER audit_log_no_update BEFORE UPDATE OR DELETE ON audit_log
    FOR EACH ROW EXECUTE FUNCTION audit_log_append_only();
CREATE TRIGGER audit_log_no_truncate BEFORE TRUNCATE ON audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION audit_log_append_only();
