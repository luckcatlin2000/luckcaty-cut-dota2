[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$DemPath,

    [string]$JobsDir = 'jobs'
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$resolvedDem = (Resolve-Path -LiteralPath $DemPath).Path
$resolvedJobs = Join-Path $projectRoot $JobsDir
$binary = Join-Path $projectRoot 'target\debug\d2-highlights.exe'

Push-Location -LiteralPath $projectRoot
try {
    if (-not (Test-Path -LiteralPath $binary)) {
        & cargo build --locked -p d2-highlights-cli
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }

    & $binary analyze $resolvedDem --jobs-dir $resolvedJobs
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
