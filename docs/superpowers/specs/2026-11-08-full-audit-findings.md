# GhostChat Full Audit Findings

| ID | Area | Severity | Finding | Failure scenario | Status | Commit |
| --- | --- | --- | --- | --- | --- | --- |
| A-1 | Build / formatting | S4 | `cargo fmt --all -- --check` fails: unformatted code in `crates/desktop/src/main.rs` (updater endpoint, ~line 60) and `crates/server/src/device_proof.rs` (import order, `ProofError` enum, `Display::fmt`). | Any CI or pre-merge step that enforces `cargo fmt --check` fails on the current branch. | fixed | 06f2c69 |
| A-2 | desktop build | S4 | crates/desktop/icons/icon.png missing; tauri::generate_context! panics on non-Windows builds so desktop tests cannot run | cargo test -p ghostchat-desktop on Linux panics at main.rs:764 | fixed | e887e8a |

## Verified checks
Run on Linux, branch `claude/dreamy-knuth-j0wr07`, after the A-1 and A-2 fixes (baseline before fixes: fmt failed, desktop crate did not compile).
Baseline run on Linux, branch `claude/dreamy-knuth-j0wr07`, before any fixes.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS (after A-1 fix) |
| `cargo test --locked -p ghostchat-server` | PASS (12 passed, 0 failed, 1 ignored) |
| `for f in scripts/test-*.cjs; do node "$f"; done` | PASS (ghost-share, media-relay, voice-gate, watch-party all exit 0) |
| `for f in crates/desktop/ui/*.js; do node --check "$f"; done` | PASS (7 files) |
| `git diff --check` | PASS |
| `cargo test --locked -p ghostchat-desktop` | PASS (2 passed, 0 failed) after A-2 fix |

### Desktop test build

Installed `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev` (after `apt-get update`). The `gdk-sys` failure is resolved, but `cargo test --locked -p ghostchat-desktop` fails with:

```
error: proc macro panicked
   --> crates/desktop/src/main.rs:764:14
764 |         .run(tauri::generate_context!())
    = help: message: failed to open icon /home/user/getchat/crates/desktop/icons/icon.png: No such file or directory (os error 2)
```

`crates/desktop/icons/` contains only `ghostchat.ico` and `ghostchat.svg`. Fixed by A-2 (PNG generated from ghostchat.svg with rsvg-convert, 512x512 RGBA; tauri.conf.json unchanged). Desktop tests now compile and pass on Linux.
