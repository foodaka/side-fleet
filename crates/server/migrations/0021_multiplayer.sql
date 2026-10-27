-- Multiplayer: machines belong to a person and run only that person's tasks,
-- runs say who started them, invite links, and rooms (a shared goal whose tasks
-- each run on their owner's machine and are merged into one room branch).
-- See docs/multiplayer/.

-- Who a machine belongs to. NULL: a machine connected with a static token
-- (self-hosted), which runs only runs nobody signed in started (developer key).
ALTER TABLE worker_tokens ADD COLUMN owner_id uuid REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE workers ADD COLUMN owner_id uuid REFERENCES users(id) ON DELETE SET NULL;

-- Who started a run: the signed-in user or the API token's user. NULL for the
-- developer key. A machine only claims runs whose creator is its owner.
ALTER TABLE runs ADD COLUMN created_by uuid REFERENCES users(id) ON DELETE SET NULL;

-- Existing data. A personal org has one member: everything there is theirs.
UPDATE worker_tokens t SET owner_id = m.user_id
  FROM org_members m
 WHERE m.org_id = t.org_id
   AND (SELECT count(*) FROM org_members c WHERE c.org_id = t.org_id) = 1;
-- A token minted by approving a waiting machine belongs to whoever approved it.
UPDATE worker_tokens t SET owner_id = e.decided_by
  FROM enrollments e
 WHERE e.token_id = t.id AND e.decided_by IS NOT NULL AND t.owner_id IS NULL;
-- Other org tokens could only be minted by owners: with one owner, it was them.
UPDATE worker_tokens t SET owner_id = (SELECT m.user_id FROM org_members m WHERE m.org_id = t.org_id AND m.role = 'owner')
 WHERE t.owner_id IS NULL
   AND (SELECT count(*) FROM org_members m WHERE m.org_id = t.org_id AND m.role = 'owner') = 1;
-- Machines take their token's owner (workers.token_hash is the token they registered with).
UPDATE workers w SET owner_id = t.owner_id
  FROM worker_tokens t
 WHERE t.token_hash = w.token_hash AND t.org_id = w.org_id;
UPDATE runs r SET created_by = m.user_id
  FROM org_members m
 WHERE m.org_id = r.org_id
   AND (SELECT count(*) FROM org_members c WHERE c.org_id = r.org_id) = 1;
-- In team orgs, a run nobody can be named for goes to its machine's owner.
UPDATE runs r SET created_by = w.owner_id
  FROM workers w
 WHERE r.created_by IS NULL AND w.org_id = r.org_id AND w.id = r.worker_id AND w.owner_id IS NOT NULL;

-- Invite links. Only the SHA-256 of the code is stored; the code is shown once.
CREATE TABLE org_invites (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    code_hash   text NOT NULL UNIQUE,
    role        text NOT NULL CHECK (role IN ('owner', 'member')),
    created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
    max_uses    integer,
    uses        integer NOT NULL DEFAULT 0,
    expires_at  timestamptz NOT NULL,
    revoked_at  timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX org_invites_org_idx ON org_invites (org_id, created_at DESC);

-- Rooms: a goal, the people's conversation about it, and the tasks that do it.
CREATE TABLE rooms (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name           text NOT NULL,
    goal           text NOT NULL DEFAULT '',
    repo           text NOT NULL,
    created_by     uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    archived_at    timestamptz,
    last_activity_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX rooms_org_idx ON rooms (org_id, last_activity_at DESC);

CREATE TABLE room_notes (
    id         uuid PRIMARY KEY,
    room_id    uuid NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    author_id  uuid REFERENCES users(id) ON DELETE SET NULL,
    body       text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX room_notes_room_idx ON room_notes (room_id, created_at);

ALTER TABLE runs ADD COLUMN room_id uuid REFERENCES rooms(id) ON DELETE SET NULL;
CREATE INDEX runs_room_idx ON runs (room_id) WHERE room_id IS NOT NULL;

