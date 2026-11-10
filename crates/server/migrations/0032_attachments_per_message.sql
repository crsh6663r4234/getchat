-- A message may carry up to eight encrypted attachments (send_message), but
-- 0026 made the message link unique, so the second link failed. The index
-- stays for the lookup by message; it only stops being unique.
DROP INDEX attachments_by_message;
CREATE INDEX attachments_by_message ON attachments(message_id) WHERE message_id IS NOT NULL;
