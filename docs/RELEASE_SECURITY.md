# Release security policy

This document describes the GhostChat Windows release pipeline. It is a release
process, not a claim that Windows, GitHub, an endpoint or a network is trusted.

## Two independent signatures

Windows Authenticode and the Tauri updater signature protect different things.

- **Authenticode** identifies the publisher of a Windows executable or MSI to
  Windows. It requires a certificate and timestamping service supplied by an
  approved signing provider. It can improve publisher attribution; it does not
  guarantee SmartScreen reputation or prove an artifact is harmless.
- **Tauri updater signing** signs the final installer bytes for the installed
  app's updater public key. It remains required even when Authenticode is used.
  The Tauri private key is an existing GitHub secret and is never printed,
  written to a file, or placed in this repository.

The order is mandatory for a signed Windows release:

1. Build the MSI without a Tauri updater signature.
2. Authenticode-sign and verify the MSI (and the separately distributed server
   executable).
3. Generate the Tauri updater signature over those final MSI bytes.
4. Upload the MSI, its `.sig`, `latest.json`, server executable and checksum.

Signing an MSI after its updater `.sig` exists changes its bytes and invalidates
the updater signature. The workflow intentionally never does that.

## Signing runner configuration

The trusted Windows signing runner must provision the certificate in either
`Cert:\CurrentUser\My` or `Cert:\LocalMachine\My`. The private key stays in the
certificate store or backing HSM/provider; do not upload a PFX to this
repository or GitHub Actions secret.

Configure only these non-secret repository/environment values:

| Name | Purpose |
| --- | --- |
| `GHOSTCHAT_REQUIRE_WINDOWS_CODESIGN` | Set to `true` for a production release. Missing signing configuration then fails the workflow. |
| `GHOSTCHAT_CODESIGN_CERT_SHA1` | SHA-1 certificate thumbprint used to select the already-provisioned certificate. |
| `GHOSTCHAT_CODESIGN_TIMESTAMP_URL` | Approved RFC 3161 timestamp service URL. |

`scripts/sign-windows.ps1` validates the certificate's private-key presence and
expiry, invokes `signtool.exe` with SHA-256 file and timestamp digests, then
checks the resulting Authenticode status. It accepts no PFX path/password.

When `GHOSTCHAT_REQUIRE_WINDOWS_CODESIGN` is not `true`, the workflow is an
**unsigned development release path**. It emits a clear warning, still requires
the Tauri updater signature, and must not be described as Authenticode-signed,
enterprise-ready, or exempt from SmartScreen. Promote a public production
release only after setting the required gate and verifying the published
artifacts.

## Operator verification

Before publishing a production release:

1. Confirm the workflow runs from a protected main branch/environment and the
   updater key is available only there.
2. Confirm `GHOSTCHAT_REQUIRE_WINDOWS_CODESIGN=true` and that the installed
   certificate thumbprint is the intended provider certificate.
3. Inspect the build log for `Authenticode verified` for both the MSI and
   `ghostchat-server-windows-x64.exe`.
4. Download the MSI and run `Get-AuthenticodeSignature` from a separate Windows
   machine; confirm `Status` is `Valid` and the expected publisher is shown.
5. Download the `.sig` and verify the updater against the public key embedded in
   the shipped app. Confirm `latest.json` points to the same MSI and signature.
6. Verify the attached server SHA-256 against its checksum file.

## Scope and remaining work

The MSI is the desktop application's Windows delivery artifact. The bundled
desktop executable is covered by the MSI's installer signature; signing an
embedded executable after MSI construction would invalidate the MSI and updater
signature. A future custom bundle-hook pipeline is required if policy mandates a
separate Authenticode signature on the embedded executable itself.

This workflow does not implement anti-rollback, updater metadata expiry,
provenance/SBOM, signing-key rotation, or an external penetration test. Those
remain separate release gates. GitHub Actions have been pinned only where a
full, verified commit SHA was available during this change; review and pin the
remaining third-party actions before relying on the workflow for high-assurance
releases.
