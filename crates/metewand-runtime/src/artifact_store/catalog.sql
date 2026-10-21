PRAGMA user_version = 1;

CREATE TABLE objects (
    tree_sha256 TEXT PRIMARY KEY NOT NULL CHECK (length(tree_sha256) = 64 AND tree_sha256 NOT GLOB '*[^0-9a-f]*'),
    manifest_sha256 TEXT NOT NULL CHECK (length(manifest_sha256) = 64 AND manifest_sha256 NOT GLOB '*[^0-9a-f]*')
) STRICT;

CREATE TABLE roots (
    name TEXT NOT NULL CHECK (length(name) > 0),
    tree_sha256 TEXT NOT NULL REFERENCES objects(tree_sha256),
    PRIMARY KEY (name, tree_sha256)
) STRICT;

-- The intent precedes the move. Cleanup starts only after the detached state
-- commits, so a crash can leave partial garbage without damaging a live object.
CREATE TABLE gc_pending (
    tree_sha256 TEXT PRIMARY KEY NOT NULL REFERENCES objects(tree_sha256),
    detached INTEGER NOT NULL CHECK (detached IN (0, 1))
) STRICT;

CREATE TRIGGER objects_no_update BEFORE UPDATE ON objects
BEGIN SELECT RAISE(ABORT, 'content identity and manifest are immutable'); END;
