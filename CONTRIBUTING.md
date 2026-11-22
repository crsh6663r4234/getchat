# Contributing to GhostChat

Thanks for helping. GhostChat is a Windows desktop app (Tauri 2) with a Rust
server (Axum, SQLite) and an encrypted group protocol (OpenMLS).

## Before you start

- Open an issue first for anything larger than a small fix, so the approach can be agreed.
- Security problems: do not open a public issue. See [SECURITY.md](SECURITY.md).

## Checks

Every change must pass these before it is submitted:

```bash
cargo fmt --all -- --check
cargo test --locked -p ghostchat-server
for f in scripts/test-*.cjs; do node "$f"; done
for f in crates/desktop/ui/*.js; do node --check "$f"; done
git diff --check
```

The desktop crate (`ghostchat-desktop`) targets Windows. On Linux it needs the
GTK and WebKit development packages to build.

## Versions

Do not edit versions by hand. Run `node scripts/bump-version.cjs X.Y.Z`, then
`node scripts/test-version-sync.cjs`. See [docs/UPDATES.md](docs/UPDATES.md).

## Pull requests

- One logical change per pull request, with a clear description of what and why.
- Add or update tests for behaviour you change.
- Keep unrelated formatting and refactors out of the change.
