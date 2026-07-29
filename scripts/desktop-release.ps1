param()

$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$DesktopRoot = Join-Path $ProjectRoot "apps\d2-highlights-desktop"
$ReleaseRoot = Join-Path $ProjectRoot "release"
$VerifyScript = Join-Path $PSScriptRoot "verify.ps1"

if ([string]::IsNullOrWhiteSpace($env:TAURI_SIGNING_PRIVATE_KEY) -or
    [string]::IsNullOrWhiteSpace($env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD)) {
    throw "Signed releases require TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD."
}

& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $VerifyScript
if ($LASTEXITCODE -ne 0) {
    throw "Project verification failed. No release was created."
}

if (-not (Test-Path -LiteralPath (Join-Path $DesktopRoot "node_modules"))) {
    Push-Location $DesktopRoot
    try {
        npm ci
    }
    finally {
        Pop-Location
    }
}

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

$Version = (Get-Item -LiteralPath $BuiltExecutable).VersionInfo.FileVersion
$ProductName = (Get-Item -LiteralPath $BuiltExecutable).VersionInfo.ProductName
if ([string]::IsNullOrWhiteSpace($ProductName)) {
    throw "The built executable does not define a product name."
}

$RootExecutable = Join-Path $ProjectRoot "${ProductName}.exe"
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

$HistoryRoot = Get-ChildItem -LiteralPath $ReleaseRoot -Directory |
    Where-Object {
        @(Get-ChildItem -LiteralPath $_.FullName -File -Filter "${ProductName}_*.exe").Count -gt 0
    } |
    Select-Object -First 1 -ExpandProperty FullName
if ([string]::IsNullOrWhiteSpace($HistoryRoot)) {
    $HistoryRoot = Join-Path $ReleaseRoot "history"
}

New-Item -ItemType Directory -Path $HistoryRoot -Force | Out-Null

if (Test-Path -LiteralPath $RootExecutable) {
    $OldVersion = (Get-Item -LiteralPath $RootExecutable).VersionInfo.FileVersion
    if ($OldVersion -ne $Version) {
        $BackupPath = Join-Path $HistoryRoot "${ProductName}_${OldVersion}.exe"
        if (Test-Path -LiteralPath $BackupPath) {
            $Timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
            $BackupPath = Join-Path $HistoryRoot "${ProductName}_${OldVersion}_${Timestamp}.exe"
        }
        Move-Item -LiteralPath $RootExecutable -Destination $BackupPath
    }
}

Copy-Item -LiteralPath $BuiltExecutable -Destination $RootExecutable -Force

$InstallerDestination = Join-Path $ReleaseRoot $BuiltInstaller.Name
$SignatureDestination = "$InstallerDestination.sig"
Copy-Item -LiteralPath $BuiltInstaller.FullName -Destination $InstallerDestination -Force
Copy-Item -LiteralPath $BuiltInstallerSignature -Destination $SignatureDestination -Force

[pscustomobject]@{
    Version = $Version
    Executable = $RootExecutable
    Installer = $InstallerDestination
    Signature = $SignatureDestination
}
