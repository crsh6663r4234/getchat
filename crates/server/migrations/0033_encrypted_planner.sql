-- Encrypted planner items for encrypted (MLS) groups. A poll, event, vote
-- or RSVP of an encrypted group is stored as opaque ciphertext in
-- `payload_ciphertext`, with the MLS epoch and key id it was sealed with;
-- its plaintext columns stay NULL. Rows of other groups, and rows of
-- encrypted groups written before this migration, keep their plaintext
-- columns and no ciphertext. The CHECK constraints keep each row one or the
-- other. `closes_at`, `closed_at`, `cancelled_at`, ids, creators and
-- timestamps stay plaintext for every row.
--
-- The plaintext columns become nullable, which SQLite cannot do in place;
-- the whole planner family is rebuilt the same way as in 0027 (rename all,
-- create, copy parent first, drop child first).
ALTER TABLE group_event_rsvps RENAME TO group_event_rsvps_0033;
ALTER TABLE group_events RENAME TO group_events_0033;
ALTER TABLE group_poll_votes RENAME TO group_poll_votes_0033;
ALTER TABLE group_poll_options RENAME TO group_poll_options_0033;
ALTER TABLE group_polls RENAME TO group_polls_0033;

CREATE TABLE group_polls (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    creator_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    question TEXT,
    closes_at TEXT,
    closed_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    payload_ciphertext BLOB,
    payload_epoch INTEGER CHECK(payload_epoch >= 0),
    payload_key_id TEXT,
    CHECK((question IS NULL) = (payload_ciphertext IS NOT NULL)),
    CHECK((payload_ciphertext IS NULL) = (payload_epoch IS NULL)),
    CHECK((payload_ciphertext IS NULL) = (payload_key_id IS NULL))
);
CREATE TABLE group_poll_options (
    id TEXT PRIMARY KEY,
    poll_id TEXT NOT NULL REFERENCES group_polls(id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    position INTEGER NOT NULL
);
CREATE TABLE group_poll_votes (
    poll_id TEXT NOT NULL REFERENCES group_polls(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    option_id TEXT REFERENCES group_poll_options(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    payload_ciphertext BLOB,
    payload_epoch INTEGER CHECK(payload_epoch >= 0),
    payload_key_id TEXT,
    PRIMARY KEY (poll_id, user_id),
    CHECK((option_id IS NULL) = (payload_ciphertext IS NOT NULL)),
    CHECK((payload_ciphertext IS NULL) = (payload_epoch IS NULL)),
    CHECK((payload_ciphertext IS NULL) = (payload_key_id IS NULL))
);
CREATE TABLE group_events (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    creator_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    title TEXT,
    details TEXT,
    scheduled_at TEXT,
    cancelled_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    payload_ciphertext BLOB,
    payload_epoch INTEGER CHECK(payload_epoch >= 0),
    payload_key_id TEXT,
    CHECK((title IS NULL) = (payload_ciphertext IS NOT NULL)),
    CHECK((title IS NULL) = (details IS NULL)),
    CHECK((title IS NULL) = (scheduled_at IS NULL)),
    CHECK((payload_ciphertext IS NULL) = (payload_epoch IS NULL)),
    CHECK((payload_ciphertext IS NULL) = (payload_key_id IS NULL))
);
CREATE TABLE group_event_rsvps (
    event_id TEXT NOT NULL REFERENCES group_events(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT CHECK(status IS NULL OR status IN ('going','maybe','declined')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    payload_ciphertext BLOB,
    payload_epoch INTEGER CHECK(payload_epoch >= 0),
    payload_key_id TEXT,
    PRIMARY KEY (event_id, user_id),
    CHECK((status IS NULL) = (payload_ciphertext IS NOT NULL)),
    CHECK((payload_ciphertext IS NULL) = (payload_epoch IS NULL)),
    CHECK((payload_ciphertext IS NULL) = (payload_key_id IS NULL))
);

INSERT INTO group_polls(id,conversation_id,creator_id,question,closes_at,closed_at,created_at) SELECT id,conversation_id,creator_id,question,closes_at,closed_at,created_at FROM group_polls_0033;
INSERT INTO group_poll_options(id,poll_id,label,position) SELECT id,poll_id,label,position FROM group_poll_options_0033;
INSERT INTO group_poll_votes(poll_id,user_id,option_id,created_at) SELECT poll_id,user_id,option_id,created_at FROM group_poll_votes_0033;
INSERT INTO group_events(id,conversation_id,creator_id,title,details,scheduled_at,cancelled_at,created_at) SELECT id,conversation_id,creator_id,title,details,scheduled_at,cancelled_at,created_at FROM group_events_0033;
INSERT INTO group_event_rsvps(event_id,user_id,status,updated_at) SELECT event_id,user_id,status,updated_at FROM group_event_rsvps_0033;

DROP TABLE group_poll_votes_0033;
DROP TABLE group_poll_options_0033;
DROP TABLE group_polls_0033;
DROP TABLE group_event_rsvps_0033;
DROP TABLE group_events_0033;

CREATE INDEX group_polls_by_conversation ON group_polls(conversation_id, created_at DESC);
CREATE INDEX group_poll_options_by_poll ON group_poll_options(poll_id, position);
CREATE INDEX group_events_by_conversation ON group_events(conversation_id, scheduled_at);
