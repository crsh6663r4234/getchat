CREATE TABLE media_revocation_outbox (
    room TEXT NOT NULL,
    account_id TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(room, account_id)
);
