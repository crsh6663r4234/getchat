CREATE TABLE crypto_devices (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    revoked_at TEXT
);
CREATE INDEX crypto_devices_user_idx ON crypto_devices(user_id, revoked_at);

CREATE TABLE crypto_key_packages (
    package_ref TEXT PRIMARY KEY NOT NULL,
    device_id TEXT NOT NULL REFERENCES crypto_devices(id) ON DELETE CASCADE,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    expires_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now','+84 days')),
    claimed_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    claimed_at TEXT
);
CREATE INDEX crypto_key_packages_available_idx
    ON crypto_key_packages(device_id, claimed_at, expires_at, created_at);

CREATE TABLE crypto_key_package_claims (
    requester_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_claim_id TEXT NOT NULL,
    target_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    package_ref TEXT NOT NULL REFERENCES crypto_key_packages(package_ref) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY(requester_id, client_claim_id)
);
