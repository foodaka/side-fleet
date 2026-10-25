-- Updates in chat: a connection (API token) can have Sidefleet tell its app when
-- a task finishes, fails, waits for approval or gets a pull request, so the
-- assistant brings it up in the chat. `notify_secret` is the receiving app's
-- credential (Hermes route secret, OpenClaw hook token, webhook signing key):
-- kept as is because every update is signed or sent with it.
ALTER TABLE api_tokens
    ADD COLUMN notify_kind text CHECK (notify_kind IN ('hermes', 'openclaw', 'webhook')),
    ADD COLUMN notify_url text,
    ADD COLUMN notify_secret text,
    ADD COLUMN notify_all boolean NOT NULL DEFAULT false,
    ADD COLUMN notify_last_at timestamptz,
    ADD COLUMN notify_last_error text;
CREATE INDEX api_tokens_notify_idx ON api_tokens (org_id) WHERE notify_url IS NOT NULL AND revoked_at IS NULL;

-- The connection that sent a message, so its app hears how the task goes.
ALTER TABLE runs ADD COLUMN api_token_id uuid REFERENCES api_tokens(id) ON DELETE SET NULL;
