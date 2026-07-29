[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$DesktopRoot = Join-Path $ProjectRoot "apps\d2-highlights-desktop"
$ConfigPath = Join-Path $DesktopRoot "src-tauri\tauri.conf.json"
$PackagePath = Join-Path $DesktopRoot "package.json"
$PolicyPath = Join-Path $DesktopRoot "src\appUpdater.ts"
$AppPath = Join-Path $DesktopRoot "src\App.tsx"
$RustPath = Join-Path $DesktopRoot "src-tauri\src\lib.rs"
$ExpectedEndpoint =
    "https://github.com/luckcatlin2000/luckcaty-cut-dota2/releases/latest/download/latest.json"

$Config = Get-Content -Raw -Encoding UTF8 -LiteralPath $ConfigPath | ConvertFrom-Json
$Package = Get-Content -Raw -Encoding UTF8 -LiteralPath $PackagePath | ConvertFrom-Json
$PolicySource = Get-Content -Raw -Encoding UTF8 -LiteralPath $PolicyPath
$AppSource = Get-Content -Raw -Encoding UTF8 -LiteralPath $AppPath
$RustSource = Get-Content -Raw -Encoding UTF8 -LiteralPath $RustPath
$ExpectedPublisher = [Text.Encoding]::UTF8.GetString(
    [Convert]::FromBase64String("54yr54yr5Y+q55So6JmO")
)
$ExpectedAuthorLabel = [Text.Encoding]::UTF8.GetString(
    [Convert]::FromBase64String("5L2c6ICF77ya54yr54yr5Y+q55So6JmO")
)

if ($Config.version -ne $Package.version) {
    throw "Updater gate failed: Tauri and frontend versions do not match."
}
if ($Config.bundle.createUpdaterArtifacts -ne $true) {
    throw "Updater gate failed: signed updater artifacts are not enabled."
}
if ($Config.bundle.publisher -ne $ExpectedPublisher) {
    throw "Updater gate failed: Windows publisher does not match the in-app author."
}

$Updater = $Config.plugins.updater
if ($null -eq $Updater) {
    throw "Updater gate failed: updater configuration is missing."
}
$Endpoints = @($Updater.endpoints)
if ($Endpoints.Count -ne 1 -or $Endpoints[0] -ne $ExpectedEndpoint) {
    throw "Updater gate failed: only the official GitHub Releases endpoint is allowed."
}
if ($Updater.dangerousInsecureTransportProtocol -eq $true) {
    throw "Updater gate failed: insecure update transport is forbidden."
}
if ($Updater.windows.installMode -ne "passive") {
    throw "Updater gate failed: Windows updates must use passive installation mode."
}

$PublicKey = [string]$Updater.pubkey
if ([string]::IsNullOrWhiteSpace($PublicKey)) {
    throw "Updater gate failed: update public key is missing."
}
try {
    $DecodedKey = [Text.Encoding]::UTF8.GetString(
        [Convert]::FromBase64String($PublicKey)
    )
}
catch {
    throw "Updater gate failed: update public key is not valid base64."
}
if (-not $DecodedKey.StartsWith("untrusted comment: minisign public key:")) {
    throw "Updater gate failed: update public key is not a Tauri minisign key."
}

if ($PolicySource -notmatch "checkOnStartup:\s*true" -or
    $PolicySource -notmatch "downloadAutomatically:\s*false" -or
    $PolicySource -notmatch "installAutomatically:\s*false") {
    throw "Updater gate failed: updates must remain optional."
}
if (-not $AppSource.Contains($ExpectedAuthorLabel)) {
    throw "Updater gate failed: the in-app author label is missing."
}
if ($RustSource -notmatch "check_for_app_update" -or
    $RustSource -notmatch "install_app_update" -or
    $RustSource -notmatch "download_and_install") {
    throw "Updater gate failed: controlled updater commands are incomplete."
}

[pscustomobject]@{
    Version = $Config.version
    Endpoint = $Endpoints[0]
    Publisher = $Config.bundle.publisher
    OptionalUpdates = $true
    SignedArtifacts = $true
}
