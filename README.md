# GhostChat

GhostChat is a Windows desktop chat app (Tauri 2) with a self-hostable Rust server. Current release: **4.0.54**.

- Anonymous accounts: a display name and a recovery key, no email, phone number or password.
- Direct and group chats with file sharing (up to 5 GB per file), replies, reactions, pins, search, invite links and a group planner (polls and events).
- One-to-one and group voice calls with screen sharing, a local noise filter (RNNoise, or experimental DeepFilterNet) and Watch Party in one-to-one calls.
- Optional, experimental MLS encryption for direct and group chats.
- A built-in private server for one PC, or your own server for everyone, which updates itself.

## Install

Download `GhostChat_<version>_x64_en-US.msi` from the [latest GitHub release](https://github.com/crsh6663r4234/getchat/releases/latest) and run it. GhostChat needs Windows 10 or 11 (x64) and the Microsoft Edge WebView2 Runtime; the MSI installs WebView2 if it is missing. The MSI is not Authenticode-signed unless the signing gate in [docs/RELEASE_SECURITY.md](docs/RELEASE_SECURITY.md) is configured, so Windows may show a SmartScreen prompt.

The installed app checks for signed updates at startup and every six hours, and from **Settings → Updates**.

> **One-time manual update.** Desktop app 4.0.48 to 4.0.53: the automatic update cannot install (the app closes and does not start again). Install `GhostChat_4.0.54_x64_en-US.msi` from the 4.0.54 release once by hand, with GhostChat closed; later updates install automatically. Standalone server 4.0.53 and older: replace `ghostchat-server-windows-x64.exe` by hand once; from 4.0.54 on it updates itself. Back up the server database and each PC's `%APPDATA%\app.ghostchat.desktop\` folder first. Updates are one-way; see [docs/UPDATES.md](docs/UPDATES.md).

Out of the box the app runs a private server on the PC (loopback only). To chat between computers, everyone sets the same server in **Settings → Server address**.

## Run your own server on Windows

1. Download `ghostchat-server-windows-x64.exe` (and check it against its `.sha256`) from the same release.
2. Put it in a folder writable by the account that runs it, set the environment variables you need, and start it.
3. Expose it over HTTPS: a reverse proxy with TLS, or for a quick test a Cloudflare tunnel. `scripts/start-free-demo-server.ps1` downloads `cloudflared` (and the server, if none is running), checks their SHA-256 and opens a temporary `https://…trycloudflare.com` address; see [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md#temporary-free-demo-from-a-windows-pc).

| Variable | Purpose |
| --- | --- |
| `GHOSTCHAT_BIND` | Listen address, default `0.0.0.0:38241`; use `127.0.0.1:38241` when the proxy or tunnel runs on the same machine. Keep the raw port closed to the internet. |
| `GHOSTCHAT_DATABASE` | SQLite file, default `ghostchat.sqlite` in the working directory; use an absolute path. |
| `GHOSTCHAT_ATTACHMENT_DIR` | Uploaded files, default `ghostchat-attachments`. |
| `GHOSTCHAT_TURN_URLS`, `GHOSTCHAT_TURN_SHARED_SECRET` | Optional coturn relay for one-to-one calls. |
| `GHOSTCHAT_LIVEKIT_URL`, `GHOSTCHAT_LIVEKIT_API_KEY`, `GHOSTCHAT_LIVEKIT_API_SECRET` | Optional LiveKit SFU for group calls and screen sharing. |
| `GHOSTCHAT_AUTO_UPDATE` | `false` turns off self-update. |
| `RUST_LOG` | Log level. |

[server.env.example](server.env.example) has placeholders; never commit real secrets.

**Self-update.** From 4.0.54 on, the Windows server checks the GitHub releases at startup and every 24 hours, installs only a newer release whose signature matches the key built into it, backs up the database first (`<database>.pre-update.bak`), waits for relayed calls to end (at most 30 minutes) and rolls back if the new version does not start. Set `GHOSTCHAT_AUTO_UPDATE=false` if a service manager restarts the server when it exits, or to choose when to upgrade. The server also backs up the database before every schema migration. Run one server process per database. Details: [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) and [docs/UPDATES.md](docs/UPDATES.md).

## Calls

Calls never connect peer-to-peer, so callers do not learn each other's network address. One-to-one calls use TURN and group calls use LiveKit when the server has them configured. Without them, or when they cannot be reached within about 8 seconds (for example behind a Cloudflare tunnel), the GhostChat server relays the audio and screen video itself over its WebSocket. **No call path is end-to-end encrypted**: media is encrypted in transit, but TURN, LiveKit, the server relay and a tunnel that terminates TLS can access it. The server does not store call media. See [docs/VOICE.md](docs/VOICE.md).

## Encryption status

Chats are stored on the server as plaintext unless encryption is enabled for them. A direct or group chat can opt in to MLS encryption after the members compare identity codes; only messages and files sent after that are encrypted, and the server still sees who talks to whom and when. Planner data and calls are not end-to-end encrypted. The MLS implementation is **experimental**: it has automated tests but has not had real multi-device validation or an independent security review, so do not rely on it for sensitive conversations. See [docs/SECURITY.md](docs/SECURITY.md).

Account data can be exported or deleted from Settings. Sessions are stored in Windows Credential Manager; losing both the session and the recovery key locks you out permanently.

## Build from source

Requirements: Rust (MSVC toolchain), the Windows SDK and WebView2; Node.js for the UI tests. On Linux the desktop crate needs the GTK/WebKit development packages. No npm install is needed to build.

```bash
cargo build --release -p ghostchat-server
cargo build --release -p ghostchat-desktop
# MSI, from crates/desktop with the Tauri CLI and WiX 3:
tauri build --bundles msi
```

Checks that must stay green:

```bash
cargo fmt --all -- --check
cargo test --locked -p ghostchat-server
cargo test --locked -p ghostchat-desktop
for f in scripts/test-*.cjs; do node "$f"; done
for f in crates/desktop/ui/*.js; do node --check "$f"; done
git diff --check
```

`scripts/test-audio-browser.cjs` needs Playwright 1.56.1 (see [docs/AUDIO_ENGINE.md](docs/AUDIO_ENGINE.md)).

## Documentation

- [Deployment](docs/DEPLOYMENT.md), [updates](docs/UPDATES.md), [troubleshooting](docs/TROUBLESHOOTING.md)
- [Voice and screen sharing](docs/VOICE.md), [voice engine](docs/AUDIO_ENGINE.md), [Watch Party](docs/WATCH_PARTY.md), [LiveKit on a Windows VDS](docs/WINDOWS_MEDIA_SETUP.md)
- [Security and privacy](docs/SECURITY.md), [crypto-event relay API](docs/CRYPTO_EVENT_RELAY.md), [SQLCipher vault module](docs/SECURE_VAULT.md), [release signing](docs/RELEASE_SECURITY.md)
- [Feature list](docs/FEATURES.md), [UI design system](docs/UI_DESIGN_SYSTEM.md), [performance](docs/PERFORMANCE.md)
- Historical plans and reports: [docs/archive](docs/archive/README.md)

## Türkçe özet

GhostChat, Windows için bir masaüstü sohbet uygulaması (Tauri 2) ve kendi bilgisayarınızda çalıştırabileceğiniz bir Rust sunucusudur. Güncel sürüm **4.0.54**.

- **Kurulum:** [GitHub sürümlerinden](https://github.com/crsh6663r4234/getchat/releases/latest) MSI dosyasını indirip çalıştırın. WebView2 gerekir; eksikse MSI kurar.
- **Bir kerelik elle güncelleme:** 4.0.48–4.0.53 masaüstü sürümleri kendini güncelleyemez; 4.0.54 MSI'ını bir kez elle kurun. 4.0.53 ve daha eski sunucularda `ghostchat-server-windows-x64.exe` dosyasını bir kez elle değiştirin. Önce yedek alın; güncellemeler geri alınamaz.
- **Kendi sunucunuz:** `ghostchat-server-windows-x64.exe` dosyasını çalıştırın, HTTPS ters vekil veya Cloudflare tüneli ile açın. 4.0.54'ten itibaren sunucu kendini imzalı sürümlerden günceller ve önce veritabanını yedekler; kapatmak için `GHOSTCHAT_AUTO_UPDATE=false`.
- **Aramalar:** Hiçbir zaman doğrudan (eşten eşe) bağlanmaz. TURN ve LiveKit isteğe bağlıdır; yoksa veya ulaşılamıyorsa ses ve ekran görüntüsü GhostChat sunucusu üzerinden aktarılır. Aramalar uçtan uca şifreli **değildir**; sunucu yöneticisi aktarılan medyaya erişebilir.
- **Şifreleme:** Sohbetler, şifreleme açılmadıkça sunucuda düz metin olarak durur. MLS şifrelemesi deneyseldir ve bağımsız güvenlik incelemesinden geçmemiştir.
- **Kaynaktan derleme ve testler:** yukarıdaki "Build from source" bölümündeki komutlar.
