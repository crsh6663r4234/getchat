# GhostChat Full Audit Findings

| ID | Area | Severity | Finding | Failure scenario | Status | Commit |
| --- | --- | --- | --- | --- | --- | --- |
| A-1 | Build / formatting | Low | `cargo fmt --all -- --check` fails: unformatted code in `crates/desktop/src/main.rs` (updater endpoint, ~line 60) and `crates/server/src/device_proof.rs` (import order, `ProofError` enum, `Display::fmt`). | Any CI or pre-merge step that enforces `cargo fmt --check` fails on the current branch. | open | |

## Verified checks

Baseline run on Linux, branch `claude/dreamy-knuth-j0wr07`, before any fixes.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | FAIL (see A-1) |
| `cargo test --locked -p ghostchat-server` | PASS (12 passed, 0 failed, 1 ignored) |
| `for f in scripts/test-*.cjs; do node "$f"; done` | PASS (ghost-share, media-relay, voice-gate, watch-party all exit 0) |
| `for f in crates/desktop/ui/*.js; do node --check "$f"; done` | PASS (7 files) |
| `git diff --check` | PASS |
| `cargo test --locked -p ghostchat-desktop` | NOT RUN to completion: GTK/WebKit dev packages installed, but compile fails (see below) |

### Desktop test build

Installed `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev` (after `apt-get update`). The `gdk-sys` failure is resolved, but `cargo test --locked -p ghostchat-desktop` fails with:

```
error: proc macro panicked
   --> crates/desktop/src/main.rs:764:14
764 |         .run(tauri::generate_context!())
    = help: message: failed to open icon /home/user/getchat/crates/desktop/icons/icon.png: No such file or directory (os error 2)
```

`crates/desktop/icons/` contains only `ghostchat.ico` and `ghostchat.svg`. Desktop Rust is therefore audited by reading only.
