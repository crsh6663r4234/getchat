# Troubleshooting

## The desktop app does not open

- Install the Microsoft Edge WebView2 Runtime if it is missing.
- Use the x64 Windows build on Windows 10 or Windows 11.
- The app and MSI are unsigned. Windows may show a SmartScreen prompt.
- If using the MSI, it installs the Microsoft WebView2 bootstrapper. If WebView2 is absent, complete its setup and then reopen GhostChat.
- If starting from source, run the app from `crates/desktop` through the Tauri CLI or build the release executable. Keep the `ui` folder beside the source configuration when developing.

## I cannot log in after reinstalling Windows

Choose **Recover** and paste the recovery key saved during account creation. If the recovery key is lost and the Windows Credential Manager session is gone, the account cannot be recovered.

## Friend search does not find someone

Both people must use the same server. Search at least two characters of the username or ID. The directory is private and does not list all accounts.

## A remote server will not connect

- Enter an `https://` URL. The client rejects insecure remote `http://` addresses.
- Confirm the reverse proxy serves HTTPS and upgrades WebSocket requests to WSS.
- Confirm the server's health endpoint responds at `/health` and its origin allows the Tauri desktop client.
- Keep the raw Axum server port private behind the reverse proxy.

## I lost access to an account

Only the recovery key can restore the account. Recovery rotates existing sessions. GhostChat cannot reset the key.

## Messages are not encrypted from the server

That is expected. TLS encrypts network traffic when configured, but the server can read messages in its database. GhostChat does not implement end-to-end encryption.

## Automatic update closes the app and nothing is installed

Builds 4.0.48 to 4.0.53 used the updater's `quiet` install mode. The MSI installs for all users and needs administrator rights, and `quiet` cannot show the Windows permission prompt, so the app closes for the update, the installer fails silently and the app does not start again. Because a found update is mandatory, the next start tries again and the app cannot be used. From 4.0.54 the updater uses `passive` mode: Windows shows the permission prompt and a progress bar, and the app restarts when the install finishes.

An installed 4.0.48 to 4.0.53 still has the old setting, so it cannot update itself to 4.0.54 or later. Install 4.0.54 once by hand from the GitHub release (`GhostChat_4.0.54_x64_en-US.msi`); later updates then install automatically.
