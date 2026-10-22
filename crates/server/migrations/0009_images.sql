-- Images from agent tool results (a screenshot the agent read, an MCP tool's
-- picture). Stored apart from run_events so event replay stays small; events
-- refer to them by id.
CREATE TABLE run_images (
    id uuid PRIMARY KEY,
    run_id uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    media_type text NOT NULL,
    data bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX run_images_run_idx ON run_images (run_id);
