-- Snooze: a thread (its root run) hidden from the list and the inbox until a time,
-- or until something new happens in it after it was snoozed.
ALTER TABLE runs ADD COLUMN snoozed_until timestamptz, ADD COLUMN snoozed_at timestamptz;
