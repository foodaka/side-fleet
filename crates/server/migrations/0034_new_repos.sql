-- New projects: someone asked a machine to start a git repo or clone one into its repos
-- folder. The machine takes the request with its next claim or heartbeat and reports back.
CREATE TABLE new_repos (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    worker_id    text NOT NULL,
    created_by   uuid REFERENCES users(id) ON DELETE SET NULL,
    -- 'init': a new repo named `name`; 'clone': `url`, into `name`.
    kind         text NOT NULL CHECK (kind IN ('init', 'clone')),
    name         text NOT NULL,
    url          text,
    -- 'pending' until the machine takes it, then 'running', then 'done' or 'failed'.
    status       text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'done', 'failed')),
    -- When done: the repo's key and its folder on the machine.
    repo         text,
    path         text,
    error        text,
    -- Done, but something short of it went wrong (the first commit, say).
    warning      text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    taken_at     timestamptz,
    finished_at  timestamptz
);
CREATE INDEX new_repos_pending ON new_repos (worker_id) WHERE status = 'pending';
