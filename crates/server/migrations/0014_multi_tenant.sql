-- Multi-tenant control plane: users sign in with GitHub, belong to orgs, and every
-- machine, run and chosen repo belongs to exactly one org. A single-tenant install
-- (SIDEFLEET_DEV_KEY / SIDEFLEET_WORKER_TOKENS) maps everything to the default org
-- below, so existing data and workers keep working unchanged.

CREATE TABLE orgs (
    id         uuid PRIMARY KEY,
    name       text NOT NULL,
    slug       text NOT NULL UNIQUE,
    plan       text NOT NULL DEFAULT 'free' CHECK (plan IN ('free', 'pro')),
    created_at timestamptz NOT NULL DEFAULT now()
);

-- The org a single-tenant install (developer key, static worker tokens) works in.
INSERT INTO orgs (id, name, slug, plan)
VALUES ('00000000-0000-0000-0000-000000000001', 'Default', 'default', 'pro');

CREATE TABLE users (
    id         uuid PRIMARY KEY,
    github_id  bigint NOT NULL UNIQUE,
    login      text NOT NULL,
    email      text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE org_members (
    org_id     uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role       text NOT NULL CHECK (role IN ('owner', 'member')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);
CREATE INDEX org_members_user_idx ON org_members (user_id);

-- Per-org worker tokens. Only the SHA-256 of the value is stored; the value is
-- shown once when it is created.
CREATE TABLE worker_tokens (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name         text NOT NULL,
    token_hash   text NOT NULL UNIQUE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    revoked_at   timestamptz,
    last_used_at timestamptz
);
CREATE INDEX worker_tokens_org_idx ON worker_tokens (org_id, created_at);

-- Browser sessions. `id` is the SHA-256 of the cookie value, so a database read
-- does not hand out live sessions.
CREATE TABLE sessions (
    id         text PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sessions_user_idx ON sessions (user_id);

-- `sidefleet connect` without a token: the machine waits here until someone in an
-- org approves it on the Machines page; the token is minted when the waiting
-- connect collects it, so its value is never stored.
CREATE TABLE enrollments (
    id            uuid PRIMARY KEY,
    code          text NOT NULL UNIQUE,
    secret_hash   text NOT NULL,
    machine_name  text NOT NULL,
    worker_id     text NOT NULL,
    capabilities  jsonb NOT NULL DEFAULT '{}'::jsonb,
    org_id        uuid REFERENCES orgs(id) ON DELETE CASCADE,
    status        text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'denied', 'collected')),
    decided_by    uuid REFERENCES users(id) ON DELETE SET NULL,
    token_id      uuid REFERENCES worker_tokens(id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL
);
CREATE INDEX enrollments_org_idx ON enrollments (org_id, created_at) WHERE status = 'pending';

-- Tenancy columns. Existing rows belong to the default org; new rows must say.
ALTER TABLE workers ADD COLUMN org_id uuid NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001' REFERENCES orgs(id) ON DELETE CASCADE;
ALTER TABLE runs ADD COLUMN org_id uuid NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001' REFERENCES orgs(id) ON DELETE CASCADE;
ALTER TABLE added_repos ADD COLUMN org_id uuid NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001' REFERENCES orgs(id) ON DELETE CASCADE;
ALTER TABLE workers ALTER COLUMN org_id DROP DEFAULT;
ALTER TABLE runs ALTER COLUMN org_id DROP DEFAULT;
ALTER TABLE added_repos ALTER COLUMN org_id DROP DEFAULT;

-- Machine ids are hostnames, so two orgs may well both have a "MacBook-Pro":
-- a worker is identified by (org, id) and runs point at it the same way.
-- A usage limit belongs to a machine, so it is keyed the same way.
ALTER TABLE worker_limits ADD COLUMN org_id uuid NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001';
ALTER TABLE worker_limits ALTER COLUMN org_id DROP DEFAULT;
ALTER TABLE worker_limits DROP CONSTRAINT worker_limits_worker_id_fkey;
ALTER TABLE worker_limits DROP CONSTRAINT worker_limits_pkey;
ALTER TABLE worker_limits ADD PRIMARY KEY (org_id, worker_id, agent);
ALTER TABLE runs DROP CONSTRAINT runs_worker_id_fkey;
ALTER TABLE workers DROP CONSTRAINT workers_pkey;
ALTER TABLE workers ADD PRIMARY KEY (org_id, id);
ALTER TABLE runs ADD CONSTRAINT runs_worker_fkey FOREIGN KEY (org_id, worker_id) REFERENCES workers(org_id, id);
ALTER TABLE worker_limits ADD CONSTRAINT worker_limits_worker_fkey
    FOREIGN KEY (org_id, worker_id) REFERENCES workers(org_id, id) ON DELETE CASCADE;

ALTER TABLE added_repos DROP CONSTRAINT added_repos_pkey;
ALTER TABLE added_repos ADD PRIMARY KEY (org_id, name);

DROP INDEX runs_queued_idx;
DROP INDEX runs_created_idx;
CREATE INDEX runs_queued_idx ON runs (org_id, created_at) WHERE status = 'queued';
CREATE INDEX runs_org_created_idx ON runs (org_id, created_at DESC);
CREATE INDEX runs_org_running_idx ON runs (org_id) WHERE status = 'running';
