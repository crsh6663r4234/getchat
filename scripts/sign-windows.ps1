[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string[]]$Path,

    [switch]$Required,

    # The certificate is deliberately resolved from the Windows certificate store.
    # A PFX, its password, and a private key must never be passed through source
    # control or written to the workflow log.
    [string]$CertificateThumbprint = $env:GHOSTCHAT_CODESIGN_CERT_SHA1,
    [string]$TimestampUrl = $env:GHOSTCHAT_CODESIGN_TIMESTAMP_URL
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Find-SignTool {
    $fromPath = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($fromPath) { return $fromPath.Source }

    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kitsRoot) {
        $candidate = Get-ChildItem -LiteralPath $kitsRoot -Recurse -Filter signtool.exe -File |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($candidate) { return $candidate.FullName }
    }
    throw "signtool.exe was not found. Install a supported Windows SDK on the trusted signing runner."
}

function Get-CodeSigningCertificate([string]$Thumbprint) {
    $normalized = ($Thumbprint -replace "[^A-Fa-f0-9]", "").ToUpperInvariant()
    if ($normalized.Length -ne 40) {
        throw "GHOSTCHAT_CODESIGN_CERT_SHA1 must be the SHA-1 thumbprint of a trusted code-signing certificate."
    }

    $certificates = @(
        Get-ChildItem -Path "Cert:\CurrentUser\My" -ErrorAction SilentlyContinue
        Get-ChildItem -Path "Cert:\LocalMachine\My" -ErrorAction SilentlyContinue
    ) | Where-Object { $_.Thumbprint -eq $normalized }
    $certificate = $certificates | Select-Object -First 1
    if (-not $certificate) { throw "The configured code-signing certificate is not installed on this runner." }
    if (-not $certificate.HasPrivateKey) { throw "The configured code-signing certificate has no private key on this runner." }
    if ($certificate.NotBefore.ToUniversalTime() -gt [DateTime]::UtcNow) { throw "The configured code-signing certificate is not valid yet." }
    if ($certificate.NotAfter.ToUniversalTime() -le [DateTime]::UtcNow) { throw "The configured code-signing certificate is expired." }
    return $certificate
}

$resolvedPaths = @($Path | ForEach-Object {
    $item = Get-Item -LiteralPath $_ -ErrorAction Stop
    if ($item.PSIsContainer) { throw "Signing target must be a file: $($_)" }
    if ($item.Extension -notin ".exe", ".msi") { throw "Only Windows executable or MSI release artifacts may be signed: $($_)" }
    $item.FullName
})

if ([string]::IsNullOrWhiteSpace($CertificateThumbprint)) {
    if ($Required) { throw "Windows Authenticode signing is required, but GHOSTCHAT_CODESIGN_CERT_SHA1 is not configured." }
    Write-Warning "No Windows code-signing certificate is configured. Development artifacts remain Authenticode-unsigned."
    return
}
if ([string]::IsNullOrWhiteSpace($TimestampUrl)) {
    throw "A configured code-signing certificate requires GHOSTCHAT_CODESIGN_TIMESTAMP_URL for RFC 3161 timestamping."
}

$certificate = Get-CodeSigningCertificate $CertificateThumbprint
$signTool = Find-SignTool
foreach ($target in $resolvedPaths) {
    & $signTool sign /sha1 $certificate.Thumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 /v $target
    if ($LASTEXITCODE -ne 0) { throw "Authenticode signing failed for $target." }

    $signature = Get-AuthenticodeSignature -LiteralPath $target
    if ($signature.Status -ne "Valid" -or $signature.SignerCertificate.Thumbprint -ne $certificate.Thumbprint) {
        throw "Authenticode verification failed for $target."
    }
    Write-Host "Authenticode verified: $target ($($certificate.Subject))"
}
