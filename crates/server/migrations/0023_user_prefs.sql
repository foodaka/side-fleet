-- Your preferences from the Settings page (task defaults, send key...), so they follow
-- you to every browser you sign in on. NULL until you first change one.
ALTER TABLE users ADD COLUMN prefs jsonb;
