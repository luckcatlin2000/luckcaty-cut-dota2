param(
    [string]$SigningRootOverride
)

$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$DesktopRoot = Join-Path $ProjectRoot "apps\d2-highlights-desktop"
$ReleaseRoot = Join-Path $ProjectRoot "release"
$VerifyScript = Join-Path $PSScriptRoot "verify.ps1"
$TauriConfigPath = Join-Path $DesktopRoot "src-tauri\tauri.conf.json"
$SigningRoot = if ([string]::IsNullOrWhiteSpace($SigningRootOverride)) {
    Join-Path $ProjectRoot ".release-secrets"
}
else {
    [IO.Path]::GetFullPath($SigningRootOverride)
}
$SigningKeyPath = Join-Path $SigningRoot "cat-cut-updater.key"
$SigningPublicKeyPath = "$SigningKeyPath.pub"
$SigningPasswordPath = Join-Path $SigningRoot "cat-cut-updater-password.clixml"
$GitHubRepository = "luckcatlin2000/luckcaty-cut-dota2"

$GitStatus = @(& git -C $ProjectRoot status --porcelain)
if ($LASTEXITCODE -ne 0) {
    throw "Unable to inspect the Git working tree."
}
if ($GitStatus.Count -gt 0) {
    throw "Candidate builds require a clean Git working tree. Commit the reviewed source first."
}

foreach ($SigningPath in @(
    $SigningKeyPath,
    $SigningPublicKeyPath,
    $SigningPasswordPath
)) {
    if (-not (Test-Path -LiteralPath $SigningPath -PathType Leaf)) {
        throw "Updater signing material is incomplete: $SigningPath"
    }
}

$TauriConfig = Get-Content -Raw -Encoding UTF8 -LiteralPath $TauriConfigPath |
    ConvertFrom-Json
$ConfiguredPublicKey = [string]$TauriConfig.plugins.updater.pubkey
$LocalPublicKey = (Get-Content -Raw -LiteralPath $SigningPublicKeyPath).Trim()
if ($ConfiguredPublicKey -ne $LocalPublicKey) {
    throw "The local updater signing key does not match the public key embedded in the app."
}

& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $VerifyScript
if ($LASTEXITCODE -ne 0) {
    throw "Project verification failed. No release candidate was created."
}

if (-not (Test-Path -LiteralPath (Join-Path $DesktopRoot "node_modules"))) {
    Push-Location $DesktopRoot
    try {
        npm install
    }
    finally {
        Pop-Location
    }
}

$SigningPassword = $null
$PasswordPointer = [IntPtr]::Zero
$PreviousSigningKey = $env:TAURI_SIGNING_PRIVATE_KEY
$PreviousSigningPassword = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
try {
    $SecureSigningPassword = Import-Clixml -LiteralPath $SigningPasswordPath
    if ($SecureSigningPassword -isnot [Security.SecureString]) {
        throw "The updater signing password cannot be decrypted for this Windows user."
    }
    $PasswordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR(
        $SecureSigningPassword
    )
    $SigningPassword = [Runtime.InteropServices.Marshal]::PtrToStringBSTR(
        $PasswordPointer
    )
    $env:TAURI_SIGNING_PRIVATE_KEY = $SigningKeyPath
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $SigningPassword

    Push-Location $DesktopRoot
    try {
        npm run tauri -- build
        if ($LASTEXITCODE -ne 0) {
            throw "Tauri release build failed."
        }
    }
    finally {
        Pop-Location
    }
}
finally {
    $env:TAURI_SIGNING_PRIVATE_KEY = $PreviousSigningKey
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $PreviousSigningPassword
    if ($PasswordPointer -ne [IntPtr]::Zero) {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($PasswordPointer)
    }
    $SigningPassword = $null
}

$BuiltExecutable = Join-Path $ProjectRoot "target\release\cat-cut-assistant.exe"
$ExecutableBytes = [System.IO.File]::ReadAllBytes($BuiltExecutable)
if ($ExecutableBytes.Length -lt 256 -or
    $ExecutableBytes[0] -ne 0x4D -or
    $ExecutableBytes[1] -ne 0x5A) {
    throw "The built executable is not a valid Windows PE file."
}
$PeOffset = [BitConverter]::ToInt32($ExecutableBytes, 0x3C)
$OptionalHeaderOffset = $PeOffset + 24
$Subsystem = [BitConverter]::ToUInt16($ExecutableBytes, $OptionalHeaderOffset + 0x44)
if ($Subsystem -ne 2) {
    throw "Expected a Windows GUI executable (PE subsystem 2), found subsystem $Subsystem."
}

$VersionInfo = (Get-Item -LiteralPath $BuiltExecutable).VersionInfo
$Version = $VersionInfo.FileVersion
$ProductName = $VersionInfo.ProductName
if ([string]::IsNullOrWhiteSpace($Version)) {
    throw "The built executable does not define a file version."
}
if ([string]::IsNullOrWhiteSpace($ProductName)) {
    throw "The built executable does not define a product name."
}

$BundleRoot = Join-Path $ProjectRoot "target\release\bundle\nsis"
$BuiltInstallers = @(
    Get-ChildItem -LiteralPath $BundleRoot -File |
        Where-Object { $_.Name -like "*_${Version}_x64-setup.exe" }
)
if ($BuiltInstallers.Count -ne 1) {
    throw "Expected exactly one ${Version} NSIS installer, found $($BuiltInstallers.Count)."
}
$BuiltInstaller = $BuiltInstallers[0]
$BuiltInstallerSignature = "$($BuiltInstaller.FullName).sig"
if (-not (Test-Path -LiteralPath $BuiltInstallerSignature -PathType Leaf)) {
    throw "The signed NSIS updater artifact was not generated."
}

$GitCommit = (& git -C $ProjectRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($GitCommit)) {
    throw "Unable to resolve the Git commit for this candidate."
}
$GitBranch = (& git -C $ProjectRoot branch --show-current).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($GitBranch)) {
    throw "Release candidates must be built from a named Git branch."
}

$CandidateRoot = Join-Path $ReleaseRoot "candidate\$Version"
$CandidateParent = [IO.Path]::GetFullPath((Join-Path $ReleaseRoot "candidate"))
$ResolvedCandidateRoot = [IO.Path]::GetFullPath($CandidateRoot)
if (-not $ResolvedCandidateRoot.StartsWith(
    $CandidateParent + [IO.Path]::DirectorySeparatorChar,
    [StringComparison]::OrdinalIgnoreCase
)) {
    throw "Candidate path escaped the release candidate directory."
}
if (Test-Path -LiteralPath $CandidateRoot) {
    Remove-Item -LiteralPath $CandidateRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $CandidateRoot -Force | Out-Null

$CandidateExecutable = Join-Path $CandidateRoot "${ProductName}_${Version}_portable.exe"
$CandidateInstaller = Join-Path $CandidateRoot $BuiltInstaller.Name
$UpdaterInstallerName = "luckcaty-cut-dota2_${Version}_x64-setup.exe"
$UpdaterInstaller = Join-Path $CandidateRoot $UpdaterInstallerName
$UpdaterSignature = "$UpdaterInstaller.sig"
Copy-Item -LiteralPath $BuiltExecutable -Destination $CandidateExecutable -Force
Copy-Item -LiteralPath $BuiltInstaller.FullName -Destination $CandidateInstaller -Force
Copy-Item -LiteralPath $BuiltInstaller.FullName -Destination $UpdaterInstaller -Force
Copy-Item -LiteralPath $BuiltInstallerSignature -Destination $UpdaterSignature -Force

$ExecutableHash = (Get-FileHash -LiteralPath $CandidateExecutable -Algorithm SHA256).Hash
$InstallerHash = (Get-FileHash -LiteralPath $CandidateInstaller -Algorithm SHA256).Hash
$UpdaterInstallerHash = (Get-FileHash -LiteralPath $UpdaterInstaller -Algorithm SHA256).Hash
$UpdaterSignatureHash = (Get-FileHash -LiteralPath $UpdaterSignature -Algorithm SHA256).Hash
$BuiltAtUtc = [DateTime]::UtcNow
$ChangelogPath = Join-Path $ProjectRoot "CHANGELOG.md"
$ChangelogLines = Get-Content -Encoding UTF8 -LiteralPath $ChangelogPath
$VersionHeadingIndex = -1
for ($Index = 0; $Index -lt $ChangelogLines.Count; $Index++) {
    if ($ChangelogLines[$Index] -match "^## .*\b$([Regex]::Escape($Version))\b") {
        $VersionHeadingIndex = $Index
        break
    }
}
if ($VersionHeadingIndex -lt 0) {
    throw "CHANGELOG.md does not contain release notes for $Version."
}
$ReleaseNoteLines = New-Object System.Collections.Generic.List[string]
for ($Index = $VersionHeadingIndex + 1; $Index -lt $ChangelogLines.Count; $Index++) {
    if ($ChangelogLines[$Index] -match "^## ") {
        break
    }
    if (-not [string]::IsNullOrWhiteSpace($ChangelogLines[$Index])) {
        $ReleaseNoteLines.Add($ChangelogLines[$Index].Trim())
    }
}
$ReleaseNotes = ($ReleaseNoteLines -join "`n").Trim()
if ([string]::IsNullOrWhiteSpace($ReleaseNotes)) {
    throw "CHANGELOG.md has no release notes for $Version."
}
$authorWord = -join @([char]0x4F5C, [char]0x8005)
$authorName = -join @(
    [char]0x732B,
    [char]0x732B,
    [char]0x53EA,
    [char]0x7528,
    [char]0x864E
)
$authorMetadataPattern = [Regex]::Escape($authorWord) +
    ".*" +
    [Regex]::Escape($authorName)
if ($ReleaseNotes -match $authorMetadataPattern) {
    throw "Release notes must not duplicate static author metadata from the application settings."
}

$UpdateSignatureContent = (Get-Content -Raw -LiteralPath $UpdaterSignature).Trim()
if ([string]::IsNullOrWhiteSpace($UpdateSignatureContent)) {
    throw "The updater signature file is empty."
}
$LatestJsonPath = Join-Path $CandidateRoot "latest.json"
$LatestJson = [ordered]@{
    version = $Version
    notes = $ReleaseNotes
    pub_date = $BuiltAtUtc.ToString("o")
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = $UpdateSignatureContent
            url = "https://github.com/$GitHubRepository/releases/download/v$Version/$UpdaterInstallerName"
        }
    }
} | ConvertTo-Json -Depth 6
[IO.File]::WriteAllText(
    $LatestJsonPath,
    $LatestJson,
    (New-Object Text.UTF8Encoding($false))
)
$LatestJsonHash = (Get-FileHash -LiteralPath $LatestJsonPath -Algorithm SHA256).Hash
$ManifestPath = Join-Path $CandidateRoot "candidate-manifest.json"

$Manifest = [ordered]@{
    schemaVersion = 2
    version = $Version
    productName = $ProductName
    publisher = [string]$TauriConfig.bundle.publisher
    gitCommit = $GitCommit
    gitBranch = $GitBranch
    builtAtUtc = $BuiltAtUtc.ToString("o")
    executable = Split-Path -Leaf $CandidateExecutable
    executableSha256 = $ExecutableHash
    installer = Split-Path -Leaf $CandidateInstaller
    installerSha256 = $InstallerHash
    updaterInstaller = Split-Path -Leaf $UpdaterInstaller
    updaterInstallerSha256 = $UpdaterInstallerHash
    updaterSignature = Split-Path -Leaf $UpdaterSignature
    updaterSignatureSha256 = $UpdaterSignatureHash
    updaterManifest = Split-Path -Leaf $LatestJsonPath
    updaterManifestSha256 = $LatestJsonHash
    updaterEndpoint = [string]$TauriConfig.plugins.updater.endpoints[0]
    peSubsystem = $Subsystem
}
$ManifestJson = $Manifest | ConvertTo-Json -Depth 6
[IO.File]::WriteAllText(
    $ManifestPath,
    $ManifestJson,
    (New-Object Text.UTF8Encoding($false))
)

[pscustomobject]@{
    Version = $Version
    GitCommit = $GitCommit
    CandidateExecutable = $CandidateExecutable
    CandidateInstaller = $CandidateInstaller
    UpdaterInstaller = $UpdaterInstaller
    UpdaterSignature = $UpdaterSignature
    UpdaterManifest = $LatestJsonPath
    Manifest = $ManifestPath
    FormalReleaseProtected = $true
}
