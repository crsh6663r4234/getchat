# GhostChat

A private chat app for Windows, with a server you can run yourself.

GhostChat is a Windows desktop app built with Tauri 2, backed by a Rust server (Axum and SQLite). Accounts need only a display name and a recovery key, no email address or phone number. Current release: **5.0.2**.

## Features

- **Anonymous accounts.** A display name and a one-time recovery key. No email, phone number or password.
- **Direct and group chats.** Replies, reactions, pins, edit and delete, search, read positions and typing indicators.
- **Files.** Photos, videos and files up to 5 GB each, stored on the server with expiring download links.
- **Groups.** Owner, admin and member roles, invite links that expire after seven days, and a planner for polls and events.
- **Voice calls.** One-to-one and group calls with push-to-talk, device selection, a local noise filter (RNNoise) and screen sharing.
- **Watch Party.** Synchronised video playback inside a one-to-one call.
- **Your own server.** Run `ghostchat-server-windows-x64.exe` on a PC or a VPS, and it updates itself from signed releases.
- **Optional end-to-end encryption (experimental).** MLS encryption for direct and group chats. See [Security](#security-and-privacy) before relying on it.

## Install

1. Download `GhostChat_<version>_x64_en-US.msi` from the [latest release](https://github.com/crsh6663r4234/getchat/releases/latest) and run it.
2. You need Windows 10 or 11 (x64). The installer adds the Microsoft Edge WebView2 Runtime if it is missing.
3. Windows may show a SmartScreen prompt, because the installer is signed for updates but is not always Authenticode-signed. See [docs/RELEASE_SECURITY.md](docs/RELEASE_SECURITY.md).

The app checks for signed updates at startup and every six hours. You can also check from **Settings → Updates**, where **Get beta updates** opts in to the beta channel ([docs/UPDATES.md](docs/UPDATES.md)).

Out of the box the app runs a private server on your PC, reachable only from that PC. To chat between computers, everyone sets the same server address in **Settings → Server address**.

## Run your own server (Windows)

1. Download `ghostchat-server-windows-x64.exe` and its `.sha256` file from the same release, and check the hash.
2. Put it in a folder that only your account can write to. Set the variables below if you need them, then start it.
3. Expose it over HTTPS with a reverse proxy that terminates TLS, or with a Cloudflare tunnel. Keep the raw port closed to the internet.

| Variable | Purpose |
| --- | --- |
| `GHOSTCHAT_BIND` | Listen address, default `0.0.0.0:38241`. Use `127.0.0.1:38241` when the proxy runs on the same machine. |
| `GHOSTCHAT_DATABASE` | SQLite file. Use an absolute path. |
| `GHOSTCHAT_ATTACHMENT_DIR` | Folder for uploaded files. |
| `GHOSTCHAT_AUTO_UPDATE` | `false` turns off self-update. |
| `GHOSTCHAT_UPDATE_CHANNEL` | `stable` (default) or `beta`. |
| `GHOSTCHAT_TURN_URLS`, `GHOSTCHAT_TURN_SHARED_SECRET` | Optional TURN relay for one-to-one calls. |
| `GHOSTCHAT_LIVEKIT_URL`, `GHOSTCHAT_LIVEKIT_API_KEY`, `GHOSTCHAT_LIVEKIT_API_SECRET` | Optional LiveKit server for group calls. |
| `RUST_LOG` | Log level. |

[`server.env.example`](server.env.example) lists the variables with placeholder values. Never commit real secrets.

The server backs up its database before every schema migration and before each self-update. Run one server process per database. Full deployment notes: [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md).

## Calls

Calls never connect peer to peer, so no participant learns another's network address. One-to-one calls use TURN and group calls use LiveKit when the server has them configured. Otherwise, or when they cannot be reached within about eight seconds, the GhostChat server relays the audio and screen video over its WebSocket. The server does not store call media. See [docs/VOICE.md](docs/VOICE.md).

## Security and privacy

- **Chats are plaintext on the server** unless you turn on encryption for that chat. The server still sees who talks to whom, and when.
- **MLS encryption is experimental.** It has automated tests, but it has not had multi-device validation or an independent security review. Do not rely on it for sensitive conversations yet.
- **Calls are not end-to-end encrypted.** Media is encrypted in transit, but TURN, LiveKit, the server relay and any proxy that terminates TLS can access it.
- **Sessions** are stored in Windows Credential Manager. If you lose both your session and your recovery key, you lose the account permanently.

Read the full threat model in [docs/SECURITY.md](docs/SECURITY.md). To report a vulnerability, use the process in [SECURITY.md](SECURITY.md); please do not open a public issue.

## Build from source

Requirements: Rust (MSVC toolchain), the Windows SDK and WebView2. Node.js is needed for the UI tests only. The desktop crate builds on Windows; on Linux it also needs the GTK and WebKit development packages.

```bash
cargo build --release -p ghostchat-server
cargo build --release -p ghostchat-desktop
```

The checks that must pass are listed in [CONTRIBUTING.md](CONTRIBUTING.md).

## Documentation

- [Deployment](docs/DEPLOYMENT.md), [updates](docs/UPDATES.md), [troubleshooting](docs/TROUBLESHOOTING.md)
- [Voice and screen sharing](docs/VOICE.md), [voice engine](docs/AUDIO_ENGINE.md), [Watch Party](docs/WATCH_PARTY.md)
- [Security and privacy](docs/SECURITY.md), [release signing](docs/RELEASE_SECURITY.md), [crypto event relay](docs/CRYPTO_EVENT_RELAY.md)
- [Feature list](docs/FEATURES.md), [UI design system](docs/UI_DESIGN_SYSTEM.md), [performance](docs/PERFORMANCE.md)

## License

MIT. See [LICENSE](LICENSE).

## Türkçe özet

GhostChat, Windows için bir sohbet uygulaması ve Rust ile yazılmış, kendi bilgisayarınızda ya da sunucunuzda çalıştırabileceğiniz bir sunucudur. Güncel sürüm **5.0.2**.

- **Kurulum:** [Sürüm sayfasından](https://github.com/crsh6663r4234/getchat/releases/latest) `GhostChat_<sürüm>_x64_en-US.msi` dosyasını indirip çalıştırın. WebView2 eksikse kurulum otomatik ekler.
- **Hesap:** E-posta ya da telefon gerekmez; yalnızca bir görünen ad ve kurtarma anahtarı.
- **Kendi sunucunuz:** `ghostchat-server-windows-x64.exe` dosyasını çalıştırın ve HTTPS ile açın. Sunucu, imzalı sürümlerden kendini günceller; kapatmak için `GHOSTCHAT_AUTO_UPDATE=false`.
- **Aramalar:** Eşten eşe bağlanmaz. Ses ve görüntü, yapılandırılmışsa TURN veya LiveKit üzerinden, yoksa GhostChat sunucusu üzerinden aktarılır. Aramalar uçtan uca şifreli **değildir**.
- **Şifreleme:** Sohbetler şifreleme açılmadıkça sunucuda düz metin olarak durur. MLS şifrelemesi deneyseldir; hassas konuşmalar için henüz kullanmayın.
- **Güvenlik açığı:** Lütfen herkese açık bir konu açmayın; [SECURITY.md](SECURITY.md) dosyasındaki yolu kullanın.
