-- Repos the developer picked to show in the composer. Names match RepoInfo.name
-- across machines; a machine still claims runs for any repo it has checked out.
CREATE TABLE added_repos (
    name text PRIMARY KEY,
    added_at timestamptz NOT NULL DEFAULT now()
);
