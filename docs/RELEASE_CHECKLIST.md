# Release checklist (Windows)

Run this before pushing a version bump to `main`. Checks marked **Windows** cannot run on Linux or in CI; they need two PCs or the installed app.

## 1. Automated checks (any machine)

```bash
cargo fmt --all -- --check
cargo test --locked -p ghostchat-server
cargo test --locked -p ghostchat-desktop      # Windows, or Linux with GTK/WebKit packages
for f in scripts/test-*.cjs; do node "$f"; done
for f in crates/desktop/ui/*.js; do node --check "$f"; done
node scripts/test-version-sync.cjs
git diff --check
```

Every command must exit 0. The one ignored server test is expected; do not un-ignore it for a release.

## 2. Version

- [ ] `node scripts/bump-version.cjs X.Y.Z` ran, and `scripts/test-version-sync.cjs` passes.
- [ ] `docs/UPDATES.md` and the README "Current version" line match `X.Y.Z`.
- [ ] Commit message starts with `release: GhostChat X.Y.Z`.

## 3. Install and update (Windows)

- [ ] Install the previous release's MSI on a clean profile. Launch it, sign in, close it.
- [ ] Install the new MSI over it. Confirm the app opens and shows the new version in Settings.
- [ ] Settings → Updates → **Check now** on the previous version offers the new one, and it installs and restarts (on builds that support the passive installer).
- [ ] Uninstall from Windows Settings. Confirm the data folder is kept or removed as the installer says.

## 4. Chat (two PCs, or one PC with two accounts on the same server)

- [ ] Send, edit and delete a message; reply; react; pin.
- [ ] Attach a photo and a file; both download on the other PC.
- [ ] Create a group, invite a friend with an invite link, and confirm the new member sees the chat.
- [ ] Search finds a message sent before the search.

## 5. Calls (two PCs, **Windows**)

- [ ] One-to-one call connects and both sides hear each other within 10 seconds.
- [ ] Group voice call: when someone speaks, their card shows **Speaking** and lights up; when they stop, it returns to listening within about half a second.
- [ ] Mute and push-to-talk: a muted member shows **Mic off** and is silent for the others.
- [ ] Through an HTTP-only tunnel (or with TURN and LiveKit unreachable), the call falls back to the server relay and audio still flows.
- [ ] Listen for gaps over a 5-minute call. Note how many, and whether they happen during a Wi-Fi hiccup.

## 6. Encryption (two PCs, **Windows**)

- [ ] Turn on encryption for a direct chat; compare identity codes; messages sent afterwards read on both sides.
- [ ] In an encrypted group, create a poll and an event on one PC, vote and RSVP on the other. Confirm the server database contains no titles (`strings` on the SQLite file).

## 7. Server (**Windows**, standalone exe)

- [ ] `GET /health` returns the new version.
- [ ] A database backup `<database>.pre-update.bak` exists after an update.
- [ ] `GHOSTCHAT_AUTO_UPDATE=false` keeps the server on its current version.

## 8. Publish

- [ ] Push to `main`. The release workflow runs and succeeds.
- [ ] The release has the MSI, its `.sig`, `latest.json`, the server exe, its `.sig` and `.sha256`.
- [ ] Download the MSI from the release page and install it on a clean profile once more.

If any step fails, stop, fix it, and start the checklist again from step 1.
