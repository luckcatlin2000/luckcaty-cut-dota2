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
    if ($changelog -match '作者.*猫猫只用虎') {
        throw 'CHANGELOG.md must not duplicate static author metadata from application settings.'
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
