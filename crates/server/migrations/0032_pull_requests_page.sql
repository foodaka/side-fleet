-- The Pull requests page: what each machine's GitHub user has authored in the repos it
-- has, as the machine last saw it. The page asks (prs_wanted_at); the next heartbeat
-- after that hands the ask to the machine (prs_asked_at), which reports back.
ALTER TABLE workers
    ADD COLUMN prs jsonb,
    ADD COLUMN prs_fetched_at timestamptz,
    ADD COLUMN prs_error text,
    ADD COLUMN prs_wanted_at timestamptz,
    ADD COLUMN prs_asked_at timestamptz;
