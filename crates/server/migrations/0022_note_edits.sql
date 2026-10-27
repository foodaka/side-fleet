-- When a room note's author last edited it; NULL if never.
ALTER TABLE room_notes ADD COLUMN edited_at timestamptz;
