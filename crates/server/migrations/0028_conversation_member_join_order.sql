-- A stored join sequence for group succession. rowid is not stable across
-- VACUUM, and founders used to be inserted sorted by user id. Existing rows
-- keep their current rowid order; every later insert gets the next number.
ALTER TABLE conversation_members ADD COLUMN joined_seq INTEGER;
UPDATE conversation_members SET joined_seq = rowid;
CREATE INDEX conversation_members_join_order ON conversation_members(conversation_id, joined_seq);
CREATE INDEX conversation_members_joined_seq ON conversation_members(joined_seq);
CREATE TRIGGER conversation_members_assign_joined_seq
AFTER INSERT ON conversation_members
WHEN NEW.joined_seq IS NULL
BEGIN
    UPDATE conversation_members
    SET joined_seq = (SELECT COALESCE(MAX(joined_seq), 0) + 1 FROM conversation_members)
    WHERE conversation_id = NEW.conversation_id AND user_id = NEW.user_id;
END;
