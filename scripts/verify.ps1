[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$desktopRoot = Join-Path $projectRoot 'apps\d2-highlights-desktop'
Push-Location -LiteralPath $projectRoot

try {
    & cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    & cargo test --locked --workspace
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    & cargo clippy --locked --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Push-Location -LiteralPath $desktopRoot
    try {
        & npm run test:highlight-rules
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
