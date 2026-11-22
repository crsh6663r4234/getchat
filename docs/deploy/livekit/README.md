# GhostChat voice server (LiveKit) on a VDS

Requirements: a VDS with a public IP, a domain pointed at it (e.g. `voice.example.com`), Docker and Caddy.

## 1. Generate keys (on the VDS, do not commit)

    openssl rand -hex 16   # use as API key
    openssl rand -hex 32   # use as API secret

## 2. Firewall (ufw)

    sudo ufw allow 80,443/tcp
    sudo ufw allow 7881/tcp
    sudo ufw allow 3478/udp
    sudo ufw allow 5349/tcp
    sudo ufw allow 50000:60000/udp

## 3. Config

Copy `livekit.yaml`, replace `REPLACE_*` (domain and the key pair from step 1). Copy `Caddyfile` the same way and install Caddy.

## 4. Start

    docker compose up -d
    sudo systemctl reload caddy

## 5. GhostChat server environment

Add to the GhostChat server env (the Windows exe or the VDS server):

    GHOSTCHAT_LIVEKIT_URL=wss://REPLACE_VOICE_DOMAIN
    GHOSTCHAT_LIVEKIT_API_KEY=REPLACE_API_KEY
    GHOSTCHAT_LIVEKIT_API_SECRET=REPLACE_API_SECRET

Restart the GhostChat server.

## Check

    curl -sS https://REPLACE_VOICE_DOMAIN   # should respond (LiveKit or Caddy)

Keep the API secret only on the servers. Never put it in the desktop app.
