CREATE TABLE crypto_group_leave_requests (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    leaver_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    resolved_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY(conversation_id, leaver_id, operation_id)
);

CREATE UNIQUE INDEX crypto_group_leave_one_pending_idx
    ON crypto_group_leave_requests(conversation_id)
    WHERE resolved_at IS NULL;
