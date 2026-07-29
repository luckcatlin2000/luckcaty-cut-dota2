[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$JobId,

    [string]$JobsDir = 'jobs'
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$jobRoot = Join-Path (Join-Path $projectRoot $JobsDir) $JobId
$director = Join-Path $jobRoot 'director\plan.json'
$timeline = Join-Path $jobRoot 'timeline\combat-events.json'
$output = Join-Path $jobRoot 'director\control-plan.json'
$binary = Join-Path $projectRoot 'target\debug\d2-highlights.exe'

if (-not (Test-Path -LiteralPath $director -PathType Leaf)) {
    throw "Director plan not found: $director"
}
if (-not (Test-Path -LiteralPath $timeline -PathType Leaf)) {
    throw "Timeline not found: $timeline"
}

Push-Location -LiteralPath $projectRoot
try {
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        & cargo build --locked -p d2-highlights-cli
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }

    & $binary control-plan $director $timeline --output $output
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
