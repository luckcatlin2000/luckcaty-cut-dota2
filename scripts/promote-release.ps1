param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern("^\d+\.\d+\.\d+$")]
    [string]$Version,

    [switch]$ConfirmPromotion
)

$ErrorActionPreference = "Stop"

if (-not $ConfirmPromotion) {
    throw "Promotion is blocked. Re-run with -ConfirmPromotion after user acceptance."
}

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ReleaseRoot = Join-Path $ProjectRoot "release"
$CandidateRoot = Join-Path $ReleaseRoot "candidate\$Version"
$ManifestPath = Join-Path $CandidateRoot "candidate-manifest.json"
$VerifyScript = Join-Path $PSScriptRoot "verify.ps1"

if (-not (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) {
    throw "Candidate manifest not found for version $Version."
}

$GitBranch = (& git -C $ProjectRoot branch --show-current).Trim()
if ($LASTEXITCODE -ne 0 -or $GitBranch -ne "main") {
    throw "Formal promotion is allowed only from the main branch."
}

$GitStatus = @(& git -C $ProjectRoot status --porcelain)
if ($LASTEXITCODE -ne 0) {
    throw "Unable to inspect the Git working tree."
}
if ($GitStatus.Count -gt 0) {
    throw "Formal promotion requires a clean Git working tree."
}

$GitCommit = (& git -C $ProjectRoot rev-parse HEAD).Trim()
$TagCommit = (& git -C $ProjectRoot rev-list -n 1 "v$Version").Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($TagCommit)) {
    throw "The required release tag v$Version does not exist."
}
if ($GitCommit -ne $TagCommit) {
    throw "Tag v$Version does not point to the current main commit."
}

$Manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $ManifestPath |
    ConvertFrom-Json
if ($Manifest.version -ne $Version) {
    throw "Candidate manifest version does not match $Version."
}
if ($Manifest.schemaVersion -ne 2) {
    throw "Candidate manifest does not contain the signed updater contract."
}
if ($Manifest.gitCommit -ne $GitCommit) {
    throw "Candidate was not built from the current tagged commit."
}
if ($Manifest.peSubsystem -ne 2) {
    throw "Candidate is not recorded as a Windows GUI executable."
}

function Resolve-CandidateFile {
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

$CandidateExecutable = Resolve-CandidateFile $CandidateRoot $Manifest.executable
$CandidateInstaller = Resolve-CandidateFile $CandidateRoot $Manifest.installer
$UpdaterInstaller = Resolve-CandidateFile $CandidateRoot $Manifest.updaterInstaller
$UpdaterSignature = Resolve-CandidateFile $CandidateRoot $Manifest.updaterSignature
$UpdaterManifest = Resolve-CandidateFile $CandidateRoot $Manifest.updaterManifest
$ExecutableHash = (Get-FileHash -LiteralPath $CandidateExecutable -Algorithm SHA256).Hash
$InstallerHash = (Get-FileHash -LiteralPath $CandidateInstaller -Algorithm SHA256).Hash
$UpdaterInstallerHash = (Get-FileHash -LiteralPath $UpdaterInstaller -Algorithm SHA256).Hash
$UpdaterSignatureHash = (Get-FileHash -LiteralPath $UpdaterSignature -Algorithm SHA256).Hash
$UpdaterManifestHash = (Get-FileHash -LiteralPath $UpdaterManifest -Algorithm SHA256).Hash
if ($ExecutableHash -ne $Manifest.executableSha256) {
    throw "Candidate executable hash does not match its manifest."
}
if ($InstallerHash -ne $Manifest.installerSha256) {
    throw "Candidate installer hash does not match its manifest."
}
if ($UpdaterInstallerHash -ne $Manifest.updaterInstallerSha256) {
    throw "Updater installer hash does not match its manifest."
}
if ($UpdaterSignatureHash -ne $Manifest.updaterSignatureSha256) {
    throw "Updater signature hash does not match its manifest."
}
if ($UpdaterManifestHash -ne $Manifest.updaterManifestSha256) {
    throw "Updater JSON hash does not match its manifest."
}
if ($UpdaterInstallerHash -ne $InstallerHash) {
    throw "Updater installer bytes do not match the reviewed candidate installer."
}

$Latest = Get-Content -Raw -Encoding UTF8 -LiteralPath $UpdaterManifest |
    ConvertFrom-Json
$WindowsUpdate = $Latest.platforms.'windows-x86_64'
$ExpectedUpdateUrl =
    "https://github.com/luckcatlin2000/luckcaty-cut-dota2/releases/download/v$Version/$($Manifest.updaterInstaller)"
$SignatureContent = (Get-Content -Raw -LiteralPath $UpdaterSignature).Trim()
if ($Latest.version -ne $Version -or
    $WindowsUpdate.url -ne $ExpectedUpdateUrl -or
    $WindowsUpdate.signature -ne $SignatureContent) {
    throw "Updater JSON does not match the reviewed version, URL, and signature."
}

$VersionInfo = (Get-Item -LiteralPath $CandidateExecutable).VersionInfo
if ($VersionInfo.FileVersion -ne $Version) {
    throw "Candidate executable file version does not match $Version."
}
if ([string]::IsNullOrWhiteSpace($VersionInfo.ProductName)) {
    throw "Candidate executable does not define a product name."
}

& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $VerifyScript
if ($LASTEXITCODE -ne 0) {
    throw "Project verification failed. Formal files were not changed."
}

$HistoryRoot = Join-Path $ReleaseRoot "history"
New-Item -ItemType Directory -Path $HistoryRoot -Force | Out-Null
$Timestamp = Get-Date -Format "yyyyMMdd-HHmmss-fff"

$RootExecutable = Join-Path $ProjectRoot "$($VersionInfo.ProductName).exe"
$RootTemporary = "$RootExecutable.new"
if (Test-Path -LiteralPath $RootTemporary) {
    [System.IO.File]::Delete($RootTemporary)
}
Copy-Item -LiteralPath $CandidateExecutable -Destination $RootTemporary

if (Test-Path -LiteralPath $RootExecutable) {
    $OldVersion = (Get-Item -LiteralPath $RootExecutable).VersionInfo.FileVersion
    $RootBackup = Join-Path $HistoryRoot "$($VersionInfo.ProductName)_${OldVersion}_${Timestamp}.exe"
    [System.IO.File]::Replace($RootTemporary, $RootExecutable, $RootBackup, $true)
}
else {
    Move-Item -LiteralPath $RootTemporary -Destination $RootExecutable
}

$InstallerDestination = Join-Path $ReleaseRoot (Split-Path -Leaf $CandidateInstaller)
$InstallerTemporary = "$InstallerDestination.new"
if (Test-Path -LiteralPath $InstallerTemporary) {
    [System.IO.File]::Delete($InstallerTemporary)
}
Copy-Item -LiteralPath $CandidateInstaller -Destination $InstallerTemporary

if (Test-Path -LiteralPath $InstallerDestination) {
    $InstallerBaseName = [System.IO.Path]::GetFileNameWithoutExtension($InstallerDestination)
    $InstallerBackup = Join-Path $HistoryRoot "${InstallerBaseName}_${Timestamp}.exe"
    [System.IO.File]::Replace(
        $InstallerTemporary,
        $InstallerDestination,
        $InstallerBackup,
        $true
    )
}
else {
    Move-Item -LiteralPath $InstallerTemporary -Destination $InstallerDestination
}

$PromotedExecutableHash = (Get-FileHash -LiteralPath $RootExecutable -Algorithm SHA256).Hash
$PromotedInstallerHash = (Get-FileHash -LiteralPath $InstallerDestination -Algorithm SHA256).Hash
if ($PromotedExecutableHash -ne $ExecutableHash -or
    $PromotedInstallerHash -ne $InstallerHash) {
    throw "Post-promotion hash verification failed."
}

$UpdateReleaseRoot = Join-Path $ReleaseRoot "updates\$Version"
New-Item -ItemType Directory -Path $UpdateReleaseRoot -Force | Out-Null
foreach ($UpdateArtifact in @(
    $UpdaterInstaller,
    $UpdaterSignature,
    $UpdaterManifest
)) {
    Copy-Item -LiteralPath $UpdateArtifact -Destination $UpdateReleaseRoot -Force
}

[pscustomobject]@{
    Version = $Version
    GitCommit = $GitCommit
    Executable = $RootExecutable
    ExecutableSha256 = $PromotedExecutableHash
    Installer = $InstallerDestination
    InstallerSha256 = $PromotedInstallerHash
    UpdateArtifacts = $UpdateReleaseRoot
}
