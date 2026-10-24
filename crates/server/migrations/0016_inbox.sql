-- The "waiting on you" inbox and its optional web push.
--
-- Whether a task needs its developer is worked out from runs and run_events; only
-- what each viewer has seen is stored. A viewer is a signed-in user, or, for the
-- developer key, the nil uuid (one shared viewer for a self-hosted install).

-- When a viewer last looked at a task (opened or dismissed it). The task shows
-- again once it starts waiting after this.
CREATE TABLE inbox_seen (
    viewer_id uuid NOT NULL,
    root_id   uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seen_at   timestamptz NOT NULL,
    PRIMARY KEY (viewer_id, root_id)
);

-- "Clear all": everything in the org that started waiting before this counts as seen.
CREATE TABLE inbox_cleared (
    viewer_id  uuid NOT NULL,
    org_id     uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    cleared_at timestamptz NOT NULL,
    PRIMARY KEY (viewer_id, org_id)
);

-- Start everyone who exists today with an empty inbox rather than every task
-- that ever finished.
INSERT INTO inbox_cleared (viewer_id, org_id, cleared_at)
VALUES ('00000000-0000-0000-0000-000000000000', '00000000-0000-0000-0000-000000000001', now());
INSERT INTO inbox_cleared (viewer_id, org_id, cleared_at)
SELECT user_id, org_id, now() FROM org_members;

-- Browsers that asked to be notified ("Notify me"). One row per push endpoint.
CREATE TABLE push_subscriptions (
    endpoint   text PRIMARY KEY,
    viewer_id  uuid NOT NULL,
    p256dh     text NOT NULL,
    auth       text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX push_subscriptions_viewer_idx ON push_subscriptions (viewer_id);
