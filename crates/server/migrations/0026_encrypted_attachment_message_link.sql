-- An encrypted attachment is linked to the e2e1 message that carries its
-- key, so deleting that message also deletes the blob (as for plaintext
-- file messages). NULL until a message claims it.
ALTER TABLE attachments ADD COLUMN message_id TEXT;
CREATE UNIQUE INDEX attachments_by_message ON attachments(message_id) WHERE message_id IS NOT NULL;
