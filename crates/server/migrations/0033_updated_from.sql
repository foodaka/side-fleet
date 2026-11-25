-- The build a machine ran when it last started an update. Back on that same build
-- afterwards, the update had nothing newer for it (it installs releases and main is
-- ahead), so the page stops offering one until its build changes.
ALTER TABLE workers ADD COLUMN updated_from text;
