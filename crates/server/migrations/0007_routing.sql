-- Least busy first: each machine's task slots, from its last heartbeat or claim.
-- NULL for workers that do not report them (no load balancing for those).
ALTER TABLE workers ADD COLUMN slots_parallel integer;
ALTER TABLE workers ADD COLUMN slots_active integer;

-- A machine whose account hit an agent's usage limit takes no tasks for that
-- agent until `until` (the reset time the agent reported, else an hour later).
CREATE TABLE worker_limits (
    worker_id text NOT NULL REFERENCES workers(id) ON DELETE CASCADE,
    agent     text NOT NULL,
    until     timestamptz NOT NULL,
    PRIMARY KEY (worker_id, agent)
);

-- How many times a run was put back in the queue (a usage limit re-queues once).
ALTER TABLE runs ADD COLUMN requeues integer NOT NULL DEFAULT 0;
