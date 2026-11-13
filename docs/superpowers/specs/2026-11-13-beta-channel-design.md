# Beta update channel

Date: 2026-11-13. Decided by the controller; owner asked to work from the shortest item to the longest.

## Problem

Every push to `main` is released to every installed desktop and every standalone server at once. A bad release reaches everyone, and the 4.0.48 updater bug showed how costly that is.

## Goal

Two channels, both signed with the same updater key:

- **stable** (default): what `main` releases today, feed `releases/latest/download/latest.json`.
- **beta**: releases built from the `beta` branch, published as GitHub *prereleases* with their own feed `releases/download/beta/latest.json`.

Stable users never see beta builds. Beta is opt-in.

## Design

1. Release workflow. Add a second trigger: push to branch `beta`. It builds the same MSI and server exe, signs them the same way, and publishes a prerelease tagged `v<version>-beta.<run_number>` marked `--prerelease`. It uploads its `latest.json` as an asset of a fixed release tag `beta` (re-created or updated each run), so the beta feed URL is stable: `https://github.com/crsh6663r4234/getchat/releases/download/beta/latest.json`. The stable job's `on.push.branches` stays `main`; the two jobs must not share a release tag. Nothing else in the stable path changes.
2. Desktop. A local setting "Beta güncellemeleri al" (Turkish, English label too), default off, stored in the app's local settings. When on, the updater endpoint is the beta feed. The endpoint is chosen at check time, not compiled in. Changing the setting takes effect on the next update check.
3. Server. Environment variable `GHOSTCHAT_UPDATE_CHANNEL` = `stable` (default) or `beta`. The self-update feed URL follows the channel. Same signature rules as now, same no-downgrade rule (a beta build with a higher version number is a valid update; the comparison stays semver on `major.minor.patch`, so a beta build must carry a plain version too: the beta workflow must NOT change the version string, it only publishes it under the prerelease tag). Consequence: a beta and a stable build of the same version are equal; the server ignores equal versions, so beta on a server needs a version bump to take effect. Document this.
4. Branch flow. `beta` is fast-forwarded from `main` by the controller; no feature work lands on `beta` directly. Promotion beta → main is the same fast-forward.

## Non-goals

No per-user channels, no staged rollout percentages, no automatic promotion.

## Tests

- Workflow: YAML parses; the beta job does not touch the stable release tag; prerelease flag set; both jobs upload distinct assets.
- Desktop (Node, main-logic): the endpoint follows the setting; default is stable; toggling changes the next check's URL; the Turkish and English labels exist.
- Server (Rust): channel selection from the env var; default stable; invalid value falls back to stable with a logged warning; the feed URL and the no-downgrade rule are unchanged.
- Manual (Windows): turn beta on, publish a beta build with a higher version, confirm the update installs and the app restarts; turn beta off, confirm stable is offered only.

## Documents

UPDATES.md (channels, how to switch, prerelease meaning), DEPLOYMENT.md (`GHOSTCHAT_UPDATE_CHANNEL`), README (one line).
