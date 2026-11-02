-- Servers a task's agent left running on its machine (processes listening on TCP in the
-- task's worktree), as the machine last reported them with its heartbeat. Stop on the page
-- sets stop_requested_at; the machine is told on its next heartbeat and reports it gone.
CREATE TABLE dev_servers (
    thread_id         uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    pid               integer NOT NULL,
    worker_id         text NOT NULL,
    ports             integer[] NOT NULL,
    command           text NOT NULL,
    started_at        timestamptz,
    stop_requested_at timestamptz,
    seen_at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (thread_id, pid)
);
CREATE INDEX dev_servers_worker ON dev_servers (worker_id);
