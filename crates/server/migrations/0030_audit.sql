-- Audit log: who did what in an org, recorded as it happens (docs/multiplayer/08).
-- Append-only: the product never updates or deletes a row. Deleting the org removes its
-- log (ON DELETE CASCADE), and an operator may prune old rows for retention.
--
-- `run_id` and `worker_id` are copies, not foreign keys: the record of a run or a
-- machine outlives the machine being removed. They make "this run's history" and "this
-- machine's history" cheap to list. `detail` is free-form JSON (redacted before it is
-- stored), so later event types (budgets, payments) need no schema change.
CREATE TABLE audit_events (
    id        bigserial PRIMARY KEY,
    org_id    uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    actor_id  uuid REFERENCES users(id) ON DELETE SET NULL,  -- NULL: developer key, a worker, the system
    actor     text NOT NULL,                                 -- login, 'developer key', 'worker:<id>', 'system'
    action    text NOT NULL,                                 -- dotted, e.g. 'run.created', 'approval.denied'
    target    text,                                          -- run id, worker id, user id, invite id, token id
    run_id    uuid,
    worker_id text,
    detail    jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(detail) = 'object'),
    at        timestamptz NOT NULL DEFAULT now()
);
-- Newest first, keyset-paged on (at, id).
CREATE INDEX audit_events_org_idx ON audit_events (org_id, at DESC, id DESC);
CREATE INDEX audit_events_run_idx ON audit_events (org_id, run_id, at DESC) WHERE run_id IS NOT NULL;
CREATE INDEX audit_events_worker_idx ON audit_events (org_id, worker_id, at DESC) WHERE worker_id IS NOT NULL;
CREATE INDEX audit_events_actor_idx ON audit_events (org_id, actor_id, at DESC) WHERE actor_id IS NOT NULL;
CREATE INDEX audit_events_action_idx ON audit_events (org_id, action, at DESC);

-- Rows never change. The one exception is the database itself clearing `actor_id` when a
-- user is deleted (ON DELETE SET NULL); `actor` still names them.
CREATE FUNCTION audit_events_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.actor_id IS NULL
       AND (NEW.id, NEW.org_id, NEW.actor, NEW.action, NEW.target, NEW.run_id, NEW.worker_id, NEW.detail, NEW.at)
           IS NOT DISTINCT FROM
           (OLD.id, OLD.org_id, OLD.actor, OLD.action, OLD.target, OLD.run_id, OLD.worker_id, OLD.detail, OLD.at)
    THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'audit_events is append-only';
END
$$;
CREATE TRIGGER audit_events_append_only BEFORE UPDATE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION audit_events_append_only();
