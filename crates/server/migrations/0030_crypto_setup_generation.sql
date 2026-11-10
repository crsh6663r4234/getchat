-- Taking over an encryption setup whose starter deleted their account is a
-- reset, never an adoption: the successor cannot know who received the old
-- Welcome. Each takeover starts a new setup generation; clients drop the
-- local group of an older generation before joining the new one.
ALTER TABLE conversation_crypto_locks ADD COLUMN setup_generation INTEGER NOT NULL DEFAULT 0;
