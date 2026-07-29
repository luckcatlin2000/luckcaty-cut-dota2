param(
    [switch]$Build
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$DesktopRoot = Join-Path $ProjectRoot "apps\d2-highlights-desktop"

Push-Location $DesktopRoot
try {
    if (-not (Test-Path -LiteralPath (Join-Path $DesktopRoot "node_modules"))) {
        npm ci
    }

    if ($Build) {
        npm run tauri build
    }
    else {
        npm run tauri dev
    }
}
finally {
    Pop-Location
}
