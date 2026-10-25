-- Personal API tokens: let a script or a chat agent (Hermes, OpenClaw, an MCP
-- client) use the developer API as one user in one org, with a scope that limits
-- what it can do. Only the SHA-256 of the value is stored; the value is shown once.
-- `user_id` is null for tokens minted with the developer key (default org).
CREATE TABLE api_tokens (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    user_id      uuid REFERENCES users(id) ON DELETE CASCADE,
    name         text NOT NULL,
    scope        text NOT NULL CHECK (scope IN ('read', 'tasks', 'full')),
    token_hash   text NOT NULL UNIQUE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    revoked_at   timestamptz,
    last_used_at timestamptz
);
CREATE INDEX api_tokens_owner_idx ON api_tokens (org_id, user_id, created_at);
