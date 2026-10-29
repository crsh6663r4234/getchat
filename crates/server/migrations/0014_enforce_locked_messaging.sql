CREATE TRIGGER encrypted_conversations_reject_plaintext_messages
BEFORE INSERT ON messages
WHEN EXISTS (
    SELECT 1 FROM conversation_crypto_locks
    WHERE conversation_id = NEW.conversation_id
)
AND substr(NEW.content, 1, 5) <> 'e2e1:'
BEGIN
    SELECT RAISE(ABORT, 'encrypted conversation only accepts MLS content');
END;

CREATE TRIGGER encrypted_conversations_reject_message_edits
BEFORE UPDATE OF content ON messages
WHEN EXISTS (
    SELECT 1 FROM conversation_crypto_locks
    WHERE conversation_id = OLD.conversation_id
)
BEGIN
    SELECT RAISE(ABORT, 'encrypted conversation does not allow message edits');
END;
