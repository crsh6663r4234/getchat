-- A relayed commit or Welcome must outlive the account that sent it: the
-- group stays alive after an account is deleted, and a member device that
-- has not fetched the event yet can never advance without it. sender_id
-- therefore loses its foreign key (like crypto_group_deleted_members.user_id);
-- events still go with their conversation and targeted events with their
-- recipient.
--
-- sqlx runs this in a transaction, so foreign_keys stays ON. Nothing
-- references crypto_events, so the table is renamed, recreated and copied.
ALTER TABLE crypto_events RENAME TO crypto_events_0029;
DROP INDEX crypto_events_conversation_sequence_idx;
DROP INDEX crypto_events_conversation_recipient_sequence_idx;

CREATE TABLE crypto_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    sender_id TEXT NOT NULL,
    client_event_id TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    recipient_id TEXT REFERENCES users(id) ON DELETE CASCADE,
    UNIQUE(conversation_id, sender_id, client_event_id)
);

INSERT INTO crypto_events(sequence,id,conversation_id,sender_id,client_event_id,payload,created_at,recipient_id)
SELECT sequence,id,conversation_id,sender_id,client_event_id,payload,created_at,recipient_id
FROM crypto_events_0029 ORDER BY sequence;

-- Clients keep a cursor on sequence, so the counter must not fall back to
-- the highest surviving row: carry the old AUTOINCREMENT value over.
DELETE FROM sqlite_sequence WHERE name='crypto_events';
INSERT INTO sqlite_sequence(name,seq)
SELECT 'crypto_events',seq FROM sqlite_sequence WHERE name='crypto_events_0029';

DROP TABLE crypto_events_0029;

CREATE INDEX crypto_events_conversation_sequence_idx
    ON crypto_events(conversation_id, sequence);
CREATE INDEX crypto_events_conversation_recipient_sequence_idx
    ON crypto_events(conversation_id, recipient_id, sequence);
