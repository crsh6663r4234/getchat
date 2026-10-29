ALTER TABLE messages
ADD COLUMN reply_to_id TEXT REFERENCES messages(id) ON DELETE SET NULL;

CREATE TABLE message_reactions (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    emoji TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY(message_id, user_id, emoji)
);

CREATE INDEX message_reactions_message_idx ON message_reactions(message_id, emoji);
