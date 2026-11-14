[CmdletBinding()]
param(
    [string]$InstallerScriptPath,
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
$repositoryRoot = Split-Path -Parent $PSScriptRoot

function Read-NsisFileTokens {
    param([string]$Text, [int]$LineNumber)

    $tokens = New-Object 'System.Collections.Generic.List[string]'
    $position = 0
    while ($position -lt $Text.Length) {
        if ([char]::IsWhiteSpace($Text[$position])) {
            $position++
            continue
        }
        if ($Text[$position] -eq ';' -or $Text[$position] -eq '#') { break }
        if ($Text[$position] -eq '"') {
            $end = $Text.IndexOf('"', $position + 1)
            if ($end -lt 0) { throw "Installer scope: unterminated quote on line $LineNumber." }
            $tokens.Add($Text.Substring($position + 1, $end - $position - 1))
            $position = $end + 1
            if ($position -lt $Text.Length -and
                -not [char]::IsWhiteSpace($Text[$position]) -and
                $Text[$position] -ne ';' -and $Text[$position] -ne '#') {
                throw "Installer scope: ambiguous quoted argument on line $LineNumber."
            }
        }
        else {
            $start = $position
            while ($position -lt $Text.Length -and -not [char]::IsWhiteSpace($Text[$position])) {
                if ($Text[$position] -eq '"') {
                    throw "Installer scope: unsupported quote inside argument on line $LineNumber."
                }
                $position++
            }
            $tokens.Add($Text.Substring($start, $position - $start))
        }
    }
    return $tokens.ToArray()
}

function Test-InstallerScope {
    param([string[]]$Lines, [string]$RepositoryRoot)

    $resourceTargets = @(
        'tools\ffmpeg\LICENSE.txt',
        'tools\ffmpeg\SOURCE.txt',
        'tools\ffmpeg\bin\ffmpeg.exe',
        'tools\ffmpeg\bin\ffprobe.exe'
    )
    $allowedSources = @{}
    $resourceCounts = @{}
    foreach ($target in $resourceTargets) {
        $absolute = [IO.Path]::GetFullPath((Join-Path $RepositoryRoot $target))
        $allowedSources[$absolute] = $target
        $resourceCounts[$target] = 0
    }
    $macroTargets = @{
        '${MAINBINARYSRCPATH}' = 'cat-cut-assistant.exe'
        '${WEBVIEW2BOOTSTRAPPERPATH}' = '$TEMP\MicrosoftEdgeWebview2Setup.exe'
        '${WEBVIEW2INSTALLERPATH}' = '$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe'
    }
    $macroCounts = @{}
    $definitionCount = 0
    $fileCount = 0
    $lineNumber = 0
    foreach ($line in $Lines) {
        $lineNumber++
        if ($line -match '^\s*!define\s+MAINBINARYNAME\b') {
            $definitionCount++
            if ($line -notmatch '^\s*!define\s+MAINBINARYNAME\s+(?:"cat-cut-assistant"|cat-cut-assistant)\s*(?:[;#].*)?$') {
                throw "Installer scope: MAINBINARYNAME must be cat-cut-assistant (line $lineNumber)."
            }
        }
        if ($line -notmatch '^\s*File\b(.*)$') { continue }
        $arguments = $Matches[1]
        $tokens = @(Read-NsisFileTokens -Text $arguments -LineNumber $lineNumber)
        $source = $null
        $outputName = $null
        $hasArchiveFlag = $false
        foreach ($token in $tokens) {
            if ($token -ieq '/a') {
                if ($hasArchiveFlag) { throw "Installer scope: repeated /a on line $lineNumber." }
                $hasArchiveFlag = $true
            }
            elseif ($token -match '^/oname=(.*)$') {
                if ($null -ne $outputName -or [string]::IsNullOrEmpty($Matches[1])) {
                    throw "Installer scope: invalid /oname on line $lineNumber."
                }
                $outputName = $Matches[1].Replace('/', '\')
            }
            elseif ($token.StartsWith('/')) {
                throw "Installer scope: unsupported File option on line $lineNumber."
            }
            else {
                if ($null -ne $source -or [string]::IsNullOrEmpty($token)) {
                    throw "Installer scope: File must have exactly one source on line $lineNumber."
                }
                $source = $token
            }
        }
        if ($null -eq $source) { throw "Installer scope: File has no source on line $lineNumber." }
        $fileCount++
        if ($macroTargets.ContainsKey($source)) {
            $expectedTarget = $macroTargets[$source]
            if ($source -eq '${MAINBINARYSRCPATH}' -and $null -eq $outputName) {
                $outputName = $expectedTarget
            }
            if ($outputName -ine $expectedTarget) {
                throw "Installer scope: macro output target is not allowed on line $lineNumber."
            }
            if ($macroCounts.ContainsKey($source)) {
                throw "Installer scope: repeated macro source on line $lineNumber."
            }
            $macroCounts[$source] = 1
            continue
        }
        if ($source.Contains('$') -or $source.IndexOfAny([char[]]'*?') -ge 0) {
            throw "Installer scope: unknown macro or wildcard source on line $lineNumber."
        }
        $absoluteSource = if ([IO.Path]::IsPathRooted($source)) {
            [IO.Path]::GetFullPath($source)
        }
        else {
            [IO.Path]::GetFullPath((Join-Path $RepositoryRoot $source))
        }
        if (-not $allowedSources.ContainsKey($absoluteSource)) {
            throw "Installer scope: unexpected payload source on line $lineNumber."
        }
        $expectedTarget = $allowedSources[$absoluteSource]
        if ($outputName -ine $expectedTarget) {
            throw "Installer scope: resource output target mismatch on line $lineNumber."
        }
        $resourceCounts[$expectedTarget]++
        if ($resourceCounts[$expectedTarget] -ne 1) {
            throw "Installer scope: duplicate resource on line $lineNumber."
        }
    }
    if ($definitionCount -ne 1) { throw 'Installer scope: exactly one MAINBINARYNAME definition is required.' }
    if (-not $macroCounts.ContainsKey('${MAINBINARYSRCPATH}')) {
        throw 'Installer scope: main executable is missing.'
    }
    foreach ($target in $resourceTargets) {
        if ($resourceCounts[$target] -ne 1) { throw "Installer scope: required resource is missing: $target." }
    }
    return [pscustomobject]@{
        MainBinary = 'cat-cut-assistant.exe'
        ResourceCount = $resourceTargets.Count
        FileDirectiveCount = $fileCount
    }
}

function Invoke-InstallerScopeSelfTest {
    $lines = @(
        '!define MAINBINARYNAME "cat-cut-assistant"',
        'File "${MAINBINARYSRCPATH}"',
        'File "/oname=$TEMP\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"',
        'File "/oname=$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"'
    )
    foreach ($target in @('tools\ffmpeg\LICENSE.txt', 'tools\ffmpeg\SOURCE.txt', 'tools\ffmpeg\bin\ffmpeg.exe', 'tools\ffmpeg\bin\ffprobe.exe')) {
        $source = Join-Path $repositoryRoot $target
        $lines += '  File /a "/oname=' + $target + '" "' + $source + '"'
    }
    $result = Test-InstallerScope -Lines $lines -RepositoryRoot $repositoryRoot
    if ($result.ResourceCount -ne 4 -or $result.FileDirectiveCount -ne 7) {
        throw 'Installer scope self-test: valid fixture count mismatch.'
    }
    $rejections = @(
        @{ Name = 'extra WTF executable'; Lines = $lines + 'File /a "/oname=maomao-wtf-director.exe" "${EXTRA_BINARY}"' },
        @{ Name = 'ordinary extra executable'; Lines = $lines + ('File "/oname=extra.exe" "' + (Join-Path $repositoryRoot 'target\release\extra.exe') + '"') },
        @{ Name = 'module manifest'; Lines = $lines + ('File "/oname=module.json" "' + (Join-Path $repositoryRoot 'module.json') + '"') },
        @{ Name = 'user task'; Lines = $lines + ('File "/oname=jobs\draft.json" "' + (Join-Path $repositoryRoot 'jobs\draft.json') + '"') },
        @{ Name = 'wrong main binary'; Lines = @($lines | ForEach-Object { $_.Replace('MAINBINARYNAME "cat-cut-assistant"', 'MAINBINARYNAME "maomao-wtf-director"') }) },
        @{ Name = 'wrong resource target'; Lines = @($lines | ForEach-Object { $_.Replace('/oname=tools\ffmpeg\LICENSE.txt', '/oname=module.json') }) },
        @{ Name = 'main macro renamed'; Lines = @($lines | ForEach-Object { if ($_ -eq 'File "${MAINBINARYSRCPATH}"') { 'File "/oname=extra.exe" "${MAINBINARYSRCPATH}"' } else { $_ } }) },
        @{ Name = 'recursive file'; Lines = $lines + 'File /r "tools\ffmpeg"' },
        @{ Name = 'duplicate resource'; Lines = $lines + $lines[4] },
        @{ Name = 'missing resource'; Lines = @($lines | Where-Object { $_ -notmatch '/oname=tools\\ffmpeg\\bin\\ffprobe\.exe' }) },
        @{ Name = 'missing main'; Lines = @($lines | Where-Object { $_ -ne 'File "${MAINBINARYSRCPATH}"' }) },
        @{ Name = 'unterminated quote'; Lines = $lines + 'File "broken' },
        @{ Name = 'multiple sources'; Lines = $lines + 'File "${MAINBINARYSRCPATH}" "${EXTRA_BINARY}"' }
    )
    foreach ($case in $rejections) {
        $rejected = $false
        try { $null = Test-InstallerScope -Lines $case.Lines -RepositoryRoot $repositoryRoot }
        catch { $rejected = $true }
        if (-not $rejected) { throw "Installer scope self-test: accepted $($case.Name)." }
    }
    Write-Output "Installer scope self-test passed: 1 valid fixture and $($rejections.Count) rejected fixtures."
}

try {
    if ($SelfTest) {
        if (-not [string]::IsNullOrWhiteSpace($InstallerScriptPath)) {
            throw 'Use -SelfTest separately from -InstallerScriptPath.'
        }
        Invoke-InstallerScopeSelfTest
    }
    else {
        if ([string]::IsNullOrWhiteSpace($InstallerScriptPath) -or
            -not (Test-Path -LiteralPath $InstallerScriptPath -PathType Leaf)) {
            throw 'Provide an existing generated NSIS script with -InstallerScriptPath.'
        }
        $lines = @(Get-Content -LiteralPath $InstallerScriptPath -Encoding UTF8)
        Test-InstallerScope -Lines $lines -RepositoryRoot $repositoryRoot
    }
}
catch {
    Write-Error -Message $_.Exception.Message -ErrorAction Continue
    exit 1
}
