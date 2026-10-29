ALTER TABLE conversation_members
ADD COLUMN role TEXT NOT NULL DEFAULT 'member'
    CHECK (role IN ('owner', 'admin', 'member'));

UPDATE conversation_members
SET role = 'owner'
WHERE conversation_id IN (SELECT id FROM conversations WHERE kind = 'group')
  AND rowid IN (
      SELECT min(cm.rowid)
      FROM conversation_members cm
      JOIN conversations c ON c.id = cm.conversation_id
      WHERE c.kind = 'group'
      GROUP BY cm.conversation_id
  );
