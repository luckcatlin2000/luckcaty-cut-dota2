[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$desktopRoot = Join-Path $projectRoot 'apps\d2-highlights-desktop'
Push-Location -LiteralPath $projectRoot

try {
    $changelog = Get-Content -Raw -Encoding UTF8 -LiteralPath (
        Join-Path $projectRoot 'CHANGELOG.md'
    )
    $authorWord = -join @([char]0x4F5C, [char]0x8005)
    $authorName = -join @(
        [char]0x732B,
        [char]0x732B,
        [char]0x53EA,
        [char]0x7528,
        [char]0x864E
    )
    $authorMetadataPattern = [Regex]::Escape($authorWord) +
        '.*' +
        [Regex]::Escape($authorName)
    if ($changelog -match $authorMetadataPattern) {
        throw 'CHANGELOG.md must not duplicate static author metadata from application settings.'
    }

    $runtimeSourceRoots = @(
        (Join-Path $projectRoot 'crates'),
        (Join-Path $projectRoot 'apps\d2-highlights-desktop\src-tauri\src')
    )
    $runtimeSourceFiles = $runtimeSourceRoots |
        ForEach-Object { Get-ChildItem -LiteralPath $_ -Recurse -File -Include '*.rs' }
    $sensitiveSteamPattern = `
        '(?i)(userdata|user_convars|loginusers(?:\.vdf)?|ssfn|steam_?id|webcookie|' +
        'access_?token|refresh_?token|password|steamguard)'
    $sensitiveSteamMatches = $runtimeSourceFiles |
        Select-String -Pattern $sensitiveSteamPattern
    if ($sensitiveSteamMatches) {
        $locations = $sensitiveSteamMatches |
            ForEach-Object { "$($_.Path):$($_.LineNumber)" } |
            Sort-Object -Unique
        throw "Runtime source must not read or retain Steam account data: $($locations -join ', ')"
    }

    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (
        Join-Path $projectRoot 'skills\dota2-replay-camera-director\scripts\validate-camera-plan.ps1'
    ) -SelfTest
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (
        Join-Path $PSScriptRoot "verify-updater.ps1"
    )
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    & cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    & cargo test --locked --workspace
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    & cargo clippy --locked --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Push-Location -LiteralPath $desktopRoot
    try {
        & npm test
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

        & npm run build
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }
    finally {
        Pop-Location
    }
}
finally {
    Pop-Location
}
