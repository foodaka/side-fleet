-- Pin: a thread (its root run) kept at the top of the list until unpinned.
ALTER TABLE runs ADD COLUMN pinned_at timestamptz;
