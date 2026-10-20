CREATE TABLE workers (
    id                text PRIMARY KEY,
    display_name      text NOT NULL,
    token_hash        text NOT NULL,
    capabilities      jsonb NOT NULL DEFAULT '{}'::jsonb,
    last_heartbeat_at timestamptz NOT NULL DEFAULT now(),
    created_at        timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE runs (
    id             uuid PRIMARY KEY,
    task           text NOT NULL,
    model          text NOT NULL,
    repo           text NOT NULL,
    status         text NOT NULL CHECK (status IN ('queued','running','succeeded','failed','stopped')),
    worker_id      text REFERENCES workers(id),
    stop_requested boolean NOT NULL DEFAULT false,
    created_at     timestamptz NOT NULL DEFAULT now(),
    started_at     timestamptz,
    finished_at    timestamptz,
    result         jsonb
);

CREATE INDEX runs_queued_idx ON runs (created_at) WHERE status = 'queued';
CREATE INDEX runs_created_idx ON runs (created_at DESC);

CREATE TABLE run_events (
    run_id  uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq     bigint NOT NULL,
    kind    text NOT NULL,
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, seq)
);
