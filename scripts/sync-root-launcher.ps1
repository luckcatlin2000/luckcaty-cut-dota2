param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern("^\d+\.\d+\.\d+$")]
    [string]$Version,

    [switch]$ConfirmRootSync
)

$ErrorActionPreference = "Stop"

if (-not $ConfirmRootSync) {
    throw "Root launcher sync is blocked. Re-run with -ConfirmRootSync after local acceptance."
}

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$CandidateRoot = Join-Path $ProjectRoot "release\candidate\$Version"
$ManifestPath = Join-Path $CandidateRoot "candidate-manifest.json"

if (-not (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) {
    throw "Candidate manifest not found for version $Version."
}

$GitStatus = @(& git -C $ProjectRoot status --porcelain)
if ($LASTEXITCODE -ne 0) {
    throw "Unable to inspect the Git working tree."
}
if ($GitStatus.Count -gt 0) {
    throw "Root launcher sync requires a clean Git working tree."
}

$Manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $ManifestPath |
    ConvertFrom-Json
if ($Manifest.version -ne $Version) {
    throw "Candidate manifest version does not match $Version."
}
if ($Manifest.schemaVersion -ne 2 -or $Manifest.peSubsystem -ne 2) {
    throw "Candidate manifest does not describe a signed Windows GUI build."
}

$GitBranch = (& git -C $ProjectRoot branch --show-current).Trim()
if ($LASTEXITCODE -ne 0 -or $GitBranch -ne $Manifest.gitBranch) {
    throw "Candidate branch does not match the current branch."
}

& git -C $ProjectRoot cat-file -e "$($Manifest.gitCommit)^{commit}"
if ($LASTEXITCODE -ne 0) {
    throw "Candidate source commit is not available in this repository."
}
& git -C $ProjectRoot merge-base --is-ancestor $Manifest.gitCommit HEAD
if ($LASTEXITCODE -ne 0) {
    throw "Candidate source commit is not an ancestor of the current branch."
}

function Resolve-LeafPath {
    param(
        [string]$Root,
        [string]$FileName
    )

    if ([string]::IsNullOrWhiteSpace($FileName) -or
        [System.IO.Path]::GetFileName($FileName) -ne $FileName) {
        throw "Candidate manifest contains an invalid file name."
    }

    $Path = Join-Path $Root $FileName
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Candidate file not found: $FileName"
    }
    return $Path
}

$CandidateExecutable = Resolve-LeafPath $CandidateRoot $Manifest.executable
$CandidateHash = (Get-FileHash -LiteralPath $CandidateExecutable -Algorithm SHA256).Hash
if ($CandidateHash -ne $Manifest.executableSha256) {
    throw "Candidate executable hash does not match its manifest."
}

$CandidateVersionInfo = (Get-Item -LiteralPath $CandidateExecutable).VersionInfo
if ($CandidateVersionInfo.FileVersion -ne $Version -or
    [string]::IsNullOrWhiteSpace($CandidateVersionInfo.ProductName)) {
    throw "Candidate executable version metadata is invalid."
}

$RootFileName = "$($CandidateVersionInfo.ProductName).exe"
if ([System.IO.Path]::GetFileName($RootFileName) -ne $RootFileName) {
    throw "Candidate product name cannot be used as a root launcher name."
}
$RootExecutable = Join-Path $ProjectRoot $RootFileName

$RootFullPath = [System.IO.Path]::GetFullPath($RootExecutable)
$RunningRoot = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object {
        -not [string]::IsNullOrWhiteSpace($_.ExecutablePath) -and
        [System.IO.Path]::GetFullPath($_.ExecutablePath) -eq $RootFullPath
    })
if ($RunningRoot.Count -gt 0) {
    throw "Close the root launcher before syncing it."
}

$HistoryRoot = Join-Path $ProjectRoot "release\history"
New-Item -ItemType Directory -Path $HistoryRoot -Force | Out-Null
$Timestamp = Get-Date -Format "yyyyMMdd-HHmmss-fff"
$RootTemporary = "$RootExecutable.new"
if (Test-Path -LiteralPath $RootTemporary) {
    [System.IO.File]::Delete($RootTemporary)
}

Copy-Item -LiteralPath $CandidateExecutable -Destination $RootTemporary
$TemporaryHash = (Get-FileHash -LiteralPath $RootTemporary -Algorithm SHA256).Hash
if ($TemporaryHash -ne $CandidateHash) {
    [System.IO.File]::Delete($RootTemporary)
    throw "Temporary launcher hash verification failed."
}

$BackupPath = $null
if (Test-Path -LiteralPath $RootExecutable -PathType Leaf) {
    $OldVersion = (Get-Item -LiteralPath $RootExecutable).VersionInfo.FileVersion
    $BackupPath = Join-Path $HistoryRoot "$($CandidateVersionInfo.ProductName)_${OldVersion}_${Timestamp}.exe"
    [System.IO.File]::Replace($RootTemporary, $RootExecutable, $BackupPath, $true)
}
else {
    Move-Item -LiteralPath $RootTemporary -Destination $RootExecutable
}

$RootHash = (Get-FileHash -LiteralPath $RootExecutable -Algorithm SHA256).Hash
$RootVersion = (Get-Item -LiteralPath $RootExecutable).VersionInfo.FileVersion
if ($RootHash -ne $CandidateHash -or $RootVersion -ne $Version) {
    if ($null -ne $BackupPath -and (Test-Path -LiteralPath $BackupPath -PathType Leaf)) {
        Copy-Item -LiteralPath $BackupPath -Destination $RootExecutable -Force
    }
    throw "Root launcher verification failed; the previous launcher was restored when available."
}

[pscustomobject]@{
    Version = $RootVersion
    Source = $CandidateExecutable
    Destination = $RootExecutable
    Sha256 = $RootHash
    PreviousLauncherBackup = $BackupPath
}
