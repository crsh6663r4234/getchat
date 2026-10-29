ALTER TABLE conversations ADD COLUMN kind TEXT NOT NULL DEFAULT 'direct'
    CHECK (kind IN ('direct', 'group'));
ALTER TABLE conversations ADD COLUMN title TEXT;

CREATE INDEX conversations_kind_idx ON conversations(kind, created_at DESC);
