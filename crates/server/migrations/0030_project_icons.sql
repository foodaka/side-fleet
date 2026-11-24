-- Project icons: the picture shown next to a repo's group in the task list, set by
-- anyone in the org and seen by all of it. One per repo key (e.g. github.com/acme/app).
CREATE TABLE project_icons (
    org_id     uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    repo       text NOT NULL,
    -- 'emoji': value is the emoji; 'letter': value is the tile colour (#rrggbb);
    -- 'image': value is a small data: URL (PNG, JPEG or WebP).
    kind       text NOT NULL CHECK (kind IN ('emoji', 'letter', 'image')),
    value      text NOT NULL,
    updated_by uuid REFERENCES users(id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, repo)
);
