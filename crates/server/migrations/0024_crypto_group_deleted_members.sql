-- Accounts that were deleted while they were members of an MLS-locked group.
-- Their MLS leaf stays in the group until an owner/admin commits its removal,
-- so remaining clients may tolerate exactly these extra leaves. user_id has
-- no foreign key because the account row is gone. removal_committed_at is
-- set once a removal commit has been accepted, so a second commit for the
-- same leaf is refused; the row itself stays so older-epoch messages still
-- decrypt.
CREATE TABLE crypto_group_deleted_members (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    deleted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    removal_committed_at TEXT,
    PRIMARY KEY(conversation_id, user_id)
);
