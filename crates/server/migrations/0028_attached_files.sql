-- Files other than images the user attached to a message (markdown, text,
-- code, PDFs). Stored apart from run_images: those are served to the page as
-- images, these only ever as downloads.
CREATE TABLE run_files (
    id uuid PRIMARY KEY,
    run_id uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    name text NOT NULL,
    media_type text NOT NULL,
    data bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX run_files_run_idx ON run_files (run_id);
-- [{id, name, media_type, bytes}], stored in run_files.
ALTER TABLE runs ADD COLUMN files jsonb NOT NULL DEFAULT '[]';
