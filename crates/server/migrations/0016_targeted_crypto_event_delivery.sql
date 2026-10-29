ALTER TABLE crypto_events
ADD COLUMN recipient_id TEXT REFERENCES users(id) ON DELETE CASCADE;

CREATE INDEX crypto_events_conversation_recipient_sequence_idx
    ON crypto_events(conversation_id, recipient_id, sequence);
