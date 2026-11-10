-- The server cannot read MLS commits, so it could accept two different
-- commits built on the same group state (for example an admin's removal of
-- a deleted account's leaf and the owner's add): members apply whichever
-- arrives first and the other author is forked. commit_seq counts the
-- accepted commits of a conversation (and setup takeovers); a membership
-- commit names the value it was built on and is refused when the
-- conversation has moved on.
ALTER TABLE conversation_crypto_locks ADD COLUMN commit_seq INTEGER NOT NULL DEFAULT 0;
