-- Images the user attached to a message: [{id, media_type, bytes}], stored in run_images.
ALTER TABLE runs ADD COLUMN images jsonb NOT NULL DEFAULT '[]';
