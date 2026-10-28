-- Personal identity survives changes to organization membership. Workers remain
-- org-scoped projections so existing run history and worker clients keep working.
ALTER TABLE worker_tokens ALTER COLUMN org_id DROP NOT NULL;
ALTER TABLE worker_tokens DROP CONSTRAINT worker_tokens_org_id_fkey;
ALTER TABLE worker_tokens ADD FOREIGN KEY (org_id) REFERENCES orgs(id) ON DELETE SET NULL;
CREATE TABLE personal_machines (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    worker_id text NOT NULL,
    home_org_id uuid REFERENCES orgs(id) ON DELETE SET NULL,
    token_hash text NOT NULL,
    display_name text NOT NULL,
    capabilities jsonb NOT NULL,
    last_heartbeat_at timestamptz NOT NULL,
    UNIQUE (token_hash, worker_id)
);
ALTER TABLE workers ADD COLUMN machine_id uuid REFERENCES personal_machines(id) ON DELETE SET NULL;
ALTER TABLE workers ADD COLUMN enabled boolean NOT NULL DEFAULT true;
-- #57 projects one physical machine into several organizations. Prefer its
-- home row, and insert exactly one identity for each connection and worker id.
INSERT INTO personal_machines (owner_id, worker_id, home_org_id, token_hash, display_name, capabilities, last_heartbeat_at)
SELECT DISTINCT ON (w.token_hash, w.id) w.owner_id, w.id, t.org_id, w.token_hash, w.display_name, w.capabilities, w.last_heartbeat_at
FROM workers w LEFT JOIN worker_tokens t ON t.token_hash = w.token_hash
WHERE w.owner_id IS NOT NULL
ORDER BY w.token_hash, w.id, (w.org_id = t.org_id) DESC NULLS LAST, w.last_heartbeat_at DESC, w.org_id;
UPDATE workers w SET machine_id = m.id FROM personal_machines m
WHERE m.token_hash = w.token_hash AND m.worker_id = w.id;
CREATE INDEX workers_machine_idx ON workers(machine_id);
CREATE INDEX workers_token_idx ON workers(token_hash, id);
CREATE INDEX personal_machines_owner_idx ON personal_machines(owner_id);
-- Retain live assignments, but do not reactivate orgs the owner has left.
UPDATE workers w SET enabled = false WHERE w.owner_id IS NOT NULL
AND NOT EXISTS (SELECT 1 FROM org_members m WHERE m.org_id = w.org_id AND m.user_id = w.owner_id);
-- A usage limit applies to the physical machine's agent account in all orgs.
INSERT INTO worker_limits (org_id, worker_id, agent, until)
SELECT w.org_id, w.id, l.agent, max(l.until)
FROM workers w JOIN workers source ON source.machine_id = w.machine_id
JOIN worker_limits l ON l.org_id = source.org_id AND l.worker_id = source.id
WHERE w.machine_id IS NOT NULL GROUP BY w.org_id, w.id, l.agent
ON CONFLICT (org_id, worker_id, agent) DO UPDATE SET until = GREATEST(worker_limits.until, EXCLUDED.until);
