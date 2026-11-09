-- The account an encrypted membership operation targets (removals), so the
-- server keeps which leaf a relayed removal commit was accepted for.
ALTER TABLE crypto_membership_operations ADD COLUMN target_id TEXT;
