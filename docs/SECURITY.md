# Security and privacy

## Identity and credentials

- No email, phone number, or conventional password is collected.
- User IDs are random UUID v4 values. Usernames are unique per server.
- Recovery keys contain 256 random bits. The server stores an Argon2 verifier and a SHA-256 lookup digest; neither is the original key. Recovery keys are returned once at account creation.
- Session tokens contain 256 random bits. Only their SHA-256 digest is stored in SQLite. Sessions expire after 30 days. Recovery invalidates prior sessions, and the server sends a revocation event to active WebSocket clients.
- Windows clients store session tokens in Windows Credential Manager. A credential target is derived from the server URL, so different servers have separate credentials.
- A newly issued recovery key is kept temporarily in a separate Credential Manager entry until the user confirms saving a backup. The client removes that temporary copy when the user continues.

## Message data

Messages are stored as normal text in SQLite. Server operators and anyone with access to the database can read them. HTTPS protects traffic in transit when configured through a production reverse proxy; HTTPS is not end-to-end encryption. Backups are not encrypted by GhostChat. Attachments and message retention controls are not implemented.

The default server binds to loopback. Remote deployments should terminate TLS at a maintained reverse proxy, keep the app server private behind a firewall, and restrict access to the SQLite file and its backups. The sample environment file contains placeholders only.

## Abuse controls and limitations

Account creation, recovery, friend requests, and message sends have process-local rate limits. These counters reset when the server restarts and do not coordinate across multiple server replicas. The app server does not implement a proxy-aware IP limiter, TURN service, abuse-report workflow, moderator roles, or account blocking.

The WebSocket accepts an initial token frame rather than putting credentials in the URL. CORS permits the Tauri desktop origin and the local development origin. Every protected HTTP action checks the session and relevant conversation/friend authorization. The app has no telemetry or paid API dependency.

## Data lifecycle

- Export: Settings downloads account profile, friends, conversations, and stored message history as JSON.
- Delete: account deletion removes that account, sessions, friend relationships, its conversations, and those messages from the active database. Copies in operator backups are not removed automatically.
- Backup: stop the server cleanly, then copy the SQLite database and protect the copy as sensitive data. Do not copy a live WAL database without using a SQLite-aware backup procedure.

This project has not had an independent security review or penetration test. No end-to-end encryption claim is made.
