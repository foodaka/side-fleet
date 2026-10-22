-- Archive: a thread (its root run) hidden from the list until unarchived.
ALTER TABLE runs ADD COLUMN archived_at timestamptz;
