-- Owner decision: a deleted account's polls and events stay in the group
-- with an anonymous ("deleted account") creator instead of being removed
-- with it; its own votes and RSVPs are still removed (user_id cascades).
-- SQLite cannot change a foreign key action in place, and sqlx runs SQLite
-- migrations in a transaction, where foreign_keys cannot be turned off.
-- So the whole planner family is rebuilt: every table is renamed (SQLite
-- rewrites the references between them to the renamed tables), new tables
-- take the original names, rows are copied parent first, and the renamed
-- tables are dropped child first, so a drop only cascades into tables that
-- are being dropped and already emptied of children.
ALTER TABLE group_event_rsvps RENAME TO group_event_rsvps_0027;
ALTER TABLE group_events RENAME TO group_events_0027;
ALTER TABLE group_poll_votes RENAME TO group_poll_votes_0027;
ALTER TABLE group_poll_options RENAME TO group_poll_options_0027;
ALTER TABLE group_polls RENAME TO group_polls_0027;

CREATE TABLE group_polls (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    creator_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    question TEXT NOT NULL,
    closes_at TEXT,
    closed_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
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
    option_id TEXT NOT NULL REFERENCES group_poll_options(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (poll_id, user_id)
);
CREATE TABLE group_events (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    creator_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    details TEXT NOT NULL DEFAULT '',
    scheduled_at TEXT NOT NULL,
    cancelled_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE TABLE group_event_rsvps (
    event_id TEXT NOT NULL REFERENCES group_events(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('going','maybe','declined')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (event_id, user_id)
);

INSERT INTO group_polls SELECT id,conversation_id,creator_id,question,closes_at,closed_at,created_at FROM group_polls_0027;
INSERT INTO group_poll_options SELECT id,poll_id,label,position FROM group_poll_options_0027;
INSERT INTO group_poll_votes SELECT poll_id,user_id,option_id,created_at FROM group_poll_votes_0027;
INSERT INTO group_events SELECT id,conversation_id,creator_id,title,details,scheduled_at,cancelled_at,created_at FROM group_events_0027;
INSERT INTO group_event_rsvps SELECT event_id,user_id,status,updated_at FROM group_event_rsvps_0027;

DROP TABLE group_poll_votes_0027;
DROP TABLE group_poll_options_0027;
DROP TABLE group_polls_0027;
DROP TABLE group_event_rsvps_0027;
DROP TABLE group_events_0027;

CREATE INDEX group_polls_by_conversation ON group_polls(conversation_id, created_at DESC);
CREATE INDEX group_poll_options_by_poll ON group_poll_options(poll_id, position);
CREATE INDEX group_events_by_conversation ON group_events(conversation_id, scheduled_at);
