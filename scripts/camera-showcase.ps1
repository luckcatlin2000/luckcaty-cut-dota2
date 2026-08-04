param(
    [Parameter(Mandatory = $true)]
    [int]$DotaPid,

    [string]$OutputRoot = "",

    [string]$Cli = "",

    [string]$Ffmpeg = "",

    [string]$Ffprobe = "",

    [string]$MovieRoot = ""
)

$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($Cli)) {
    $Cli = Join-Path $projectRoot "target\debug\d2-highlights.exe"
}
if ([string]::IsNullOrWhiteSpace($Ffmpeg)) {
    $Ffmpeg = if ($env:FFMPEG_EXE) {
        $env:FFMPEG_EXE
    }
    else {
        Join-Path $projectRoot "tools\ffmpeg\bin\ffmpeg.exe"
    }
}
if ([string]::IsNullOrWhiteSpace($Ffprobe)) {
    $Ffprobe = if ($env:FFPROBE_EXE) {
        $env:FFPROBE_EXE
    }
    else {
        Join-Path $projectRoot "tools\ffmpeg\bin\ffprobe.exe"
    }
}
if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutputRoot = Join-Path $projectRoot "jobs-v18-camera-showcase\mirana-arrow-$stamp"
}

$movieRoot = $MovieRoot
$frameRate = 30
$sourceStartSeconds = 1107.5
$sourceEndSeconds = 1114.5
$sourceDurationSeconds = $sourceEndSeconds - $sourceStartSeconds
$sourceStartTick = 32566
$sourceEndTick = 32776
$captureStartTick = $sourceStartTick - $frameRate
$prerollFrames = $frameRate
$outputFrames = [int][Math]::Round($sourceDurationSeconds * $frameRate)
$captureFrames = $prerollFrames + $outputFrames

foreach ($required in @($Cli, $Ffmpeg, $Ffprobe)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "缺少工具：$required"
    }
}

$dota = Get-Process -Id $DotaPid -ErrorAction Stop
if ($dota.ProcessName -ne "dota2") {
    throw "PID $DotaPid 不是 Dota 2。"
}
if ([string]::IsNullOrWhiteSpace($movieRoot)) {
    $dotaExe = $dota.Path
    if ([string]::IsNullOrWhiteSpace($dotaExe)) {
        throw "无法从指定 PID 解析 Dota 2 路径，请显式传入 -MovieRoot。"
    }
    $gameRoot = Split-Path -Parent (
        Split-Path -Parent (Split-Path -Parent $dotaExe)
    )
    $movieRoot = Join-Path $gameRoot "dota\movie"
}
$otherDota = Get-Process dota2 -ErrorAction SilentlyContinue |
    Where-Object { $_.Id -ne $DotaPid }
if ($otherDota) {
    throw "检测到其他 Dota 2 进程，拒绝继续：$($otherDota.Id -join ', ')"
}

$outputDir = Join-Path $OutputRoot "独立素材"
$rawRoot = Join-Path $OutputRoot "原生帧"
$previewDir = Join-Path $OutputRoot "预览图"
New-Item -ItemType Directory -Force -Path $outputDir, $rawRoot, $previewDir | Out-Null

function Invoke-VConsole {
    param([string[]]$Commands)

    $arguments = @("vconsole-exec", "--timeout-seconds", "10")
    foreach ($command in $Commands) {
        $arguments += @("--command", $command)
    }
    $result = & $Cli @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "VConsole 命令失败：$($Commands -join '; ')"
    }
    return ($result | Out-String | ConvertFrom-Json)
}

function Get-MovieFiles {
    if (-not (Test-Path -LiteralPath $movieRoot -PathType Container)) {
        return @()
    }
    return @(Get-ChildItem -LiteralPath $movieRoot -Recurse -File -ErrorAction Stop)
}

function Wait-ForFrameCount {
    param(
        [string]$Prefix,
        [int]$RequiredCount,
        [int]$TimeoutSeconds = 120
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $frames = @(Get-MovieFiles |
            Where-Object {
                $_.Extension -ieq ".jpg" -and
                $_.Name -like "*$Prefix*"
            })
        if ($frames.Count -ge $RequiredCount) {
            return $frames.Count
        }
        if (-not (Get-Process -Id $DotaPid -ErrorAction SilentlyContinue)) {
            throw "Dota 2 在录制过程中退出。"
        }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $deadline)

    throw "等待 $Prefix 的第 $RequiredCount 帧超时。"
}

function Copy-And-EncodeCapture {
    param(
        [string]$Prefix,
        [string]$ShotId,
        [string]$Label
    )

    $shotRaw = Join-Path $rawRoot $ShotId
    New-Item -ItemType Directory -Force -Path $shotRaw | Out-Null

    $captured = @(Get-MovieFiles |
        Where-Object { $_.Name -like "*$Prefix*" } |
        Sort-Object FullName)
    $frames = @($captured | Where-Object { $_.Extension -ieq ".jpg" })
    $wav = $captured | Where-Object { $_.Extension -ieq ".wav" } | Select-Object -First 1
    if ($frames.Count -lt $captureFrames) {
        throw "$ShotId 原生帧不足：需要 $captureFrames，实际 $($frames.Count)。"
    }
    if (-not $wav) {
        throw "$ShotId 没有生成 WAV。"
    }

    $selected = @($frames | Select-Object -Skip $prerollFrames -First $outputFrames)
    for ($index = 0; $index -lt $selected.Count; $index++) {
        $destination = Join-Path $shotRaw ("frame-{0:D6}.jpg" -f ($index + 1))
        Copy-Item -LiteralPath $selected[$index].FullName -Destination $destination
    }
    $wavDestination = Join-Path $shotRaw "game.wav"
    Copy-Item -LiteralPath $wav.FullName -Destination $wavDestination

    $outputPath = Join-Path $outputDir "${ShotId}_${Label}.mp4"
    $pattern = Join-Path $shotRaw "frame-%06d.jpg"
    & $Ffmpeg -y `
        -framerate $frameRate `
        -start_number 1 `
        -i $pattern `
        -ss 1.000 `
        -i $wavDestination `
        -t ("{0:F3}" -f $sourceDurationSeconds) `
        -vf "scale=1920:1080:force_original_aspect_ratio=decrease:flags=lanczos,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:black,fps=30,format=yuv420p" `
        -c:v libx264 -preset medium -crf 18 `
        -c:a aac -b:a 192k -ar 48000 `
        -movflags +faststart `
        $outputPath
    if ($LASTEXITCODE -ne 0) {
        throw "$ShotId 编码失败。"
    }

    $previewPath = Join-Path $previewDir "${ShotId}_${Label}.jpg"
    & $Ffmpeg -y -ss 3.5 -i $outputPath -frames:v 1 -q:v 2 $previewPath | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "$ShotId 预览图生成失败。"
    }

    foreach ($path in $captured) {
        Remove-Item -LiteralPath $path.FullName -Force
    }

    return $outputPath
}

function Capture-Shot {
    param(
        [string]$ShotId,
        [string]$Label,
        [string[]]$SetupCommands,
        [object[]]$Cues = @()
    )

    Write-Host "[$ShotId] 跳转并设置 $Label"
    Invoke-VConsole @(
        "endmovie",
        "demo_resume",
        "demo_goto $captureStartTick absolute pause"
    ) | Out-Null
    Start-Sleep -Seconds 12
    Invoke-VConsole @("demo_pause") | Out-Null

    $cleanHud = @(
        "sv_cheats 1",
        "dota_spectator_hudhide",
        "dota_hud_hide_mainhud 1",
        "dota_hud_hide_topbar 1",
        "dota_hud_hide_minimap 1",
        "dota_hud_hide_overlaymap 1",
        "dota_show_itempickups 0",
        "r_draw_selected_ring 0",
        "cl_drawhud 0",
        "dota_hide_cursor 1",
        "r_drawpanorama 0"
    )
    Invoke-VConsole ($cleanHud + $SetupCommands) | Out-Null
    Start-Sleep -Seconds 1

    $prefix = "d2h_showcase_$($ShotId.Replace('-', '_'))_$(Get-Date -Format 'HHmmss')"
    Invoke-VConsole @(
        "startmovie $prefix jpg wav jpeg_quality 95 framerate $frameRate",
        "demo_pauseatservertick $sourceEndTick",
        "demo_resume"
    ) | Out-Null

    foreach ($cue in @($Cues | Sort-Object frame)) {
        Wait-ForFrameCount -Prefix $prefix -RequiredCount ([int]$cue.frame) | Out-Null
        Invoke-VConsole ([string[]]$cue.commands) | Out-Null
    }
    Wait-ForFrameCount -Prefix $prefix -RequiredCount $captureFrames | Out-Null
    Invoke-VConsole @("endmovie", "demo_pause") | Out-Null
    Start-Sleep -Seconds 2

    return Copy-And-EncodeCapture -Prefix $prefix -ShotId $ShotId -Label $Label
}

$shots = @(
    [pscustomobject]@{
        id = "S001-A"
        label = "玩家视角"
        setup = @(
            "dota_camera_distance 1200",
            "dota_spectator_hero_index 9",
            "dota_spectator_mode 3",
            "dota_camera_focus_player 9"
        )
        cues = @()
        purpose = "保留猫猫只用虎当时的正常操作、判断和镜头移动。"
    },
    [pscustomobject]@{
        id = "S001-B"
        label = "英雄跟随近景"
        setup = @(
            "dota_camera_distance 520",
            "dota_spectator_hero_index 9",
            "dota_spectator_mode 2",
            "dota_camera_focus_player 9"
        )
        cues = @()
        purpose = "持续跟随米拉娜并缩短距离，观察英雄动作与命中反馈。"
    },
    [pscustomobject]@{
        id = "S001-C"
        label = "高空俯视"
        setup = @(
            "dota_camera_allow_freecam 1",
            "dota_camera_distance 2400",
            "dota_spectator_mode 1",
            "dota_camera_set_lookatpos 1600 -80"
        )
        cues = @()
        purpose = "展示米拉娜、神箭目标与周围战场关系。"
    },
    [pscustomobject]@{
        id = "S001-D"
        label = "推进跟移"
        setup = @(
            "dota_camera_allow_freecam 1",
            "dota_camera_distance 1700",
            "dota_spectator_mode 1",
            "dota_camera_set_lookatpos 1500 -120"
        )
        cues = @(
            [pscustomobject]@{
                frame = 75
                commands = @(
                    "dota_camera_lerp_position 1720 -60",
                    "dota_camera_distance 1450"
                )
            },
            [pscustomobject]@{
                frame = 120
                commands = @(
                    "dota_camera_lerp_position 1880 80",
                    "dota_camera_distance 1150"
                )
            },
            [pscustomobject]@{
                frame = 165
                commands = @(
                    "dota_camera_lerp_position 1740 -20",
                    "dota_camera_distance 900"
                )
            },
            [pscustomobject]@{
                frame = 210
                commands = @(
                    "dota_camera_lerp_position 1450 -140",
                    "dota_camera_distance 720"
                )
            }
        )
        purpose = "按事件节奏从战场全景推进到米拉娜附近，并随追击方向移动。"
    }
)

$rendered = @()
try {
    foreach ($shot in $shots) {
        $path = Capture-Shot `
            -ShotId $shot.id `
            -Label $shot.label `
            -SetupCommands $shot.setup `
            -Cues $shot.cues
        $rendered += $path
    }

    $font = "C\:/Windows/Fonts/msyh.ttc"
    $gridPath = Join-Path $OutputRoot "米拉娜_四镜头同步对照.mp4"
    $gridFilter = @"
[0:v]scale=960:540,drawbox=x=0:y=0:w=iw:h=58:color=black@0.66:t=fill,drawtext=fontfile='$font':text='A  玩家视角':fontcolor=white:fontsize=28:x=24:y=14[v0];
[1:v]scale=960:540,drawbox=x=0:y=0:w=iw:h=58:color=black@0.66:t=fill,drawtext=fontfile='$font':text='B  英雄跟随近景':fontcolor=white:fontsize=28:x=24:y=14[v1];
[2:v]scale=960:540,drawbox=x=0:y=0:w=iw:h=58:color=black@0.66:t=fill,drawtext=fontfile='$font':text='C  高空俯视':fontcolor=white:fontsize=28:x=24:y=14[v2];
[3:v]scale=960:540,drawbox=x=0:y=0:w=iw:h=58:color=black@0.66:t=fill,drawtext=fontfile='$font':text='D  推进跟移':fontcolor=white:fontsize=28:x=24:y=14[v3];
[v0][v1]hstack=inputs=2[top];
[v2][v3]hstack=inputs=2[bottom];
[top][bottom]vstack=inputs=2[v]
"@ -replace "`r?`n", ""
    & $Ffmpeg -y `
        -i $rendered[0] -i $rendered[1] -i $rendered[2] -i $rendered[3] `
        -filter_complex $gridFilter `
        -map "[v]" -map "0:a:0" `
        -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p `
        -c:a aac -b:a 192k -ar 48000 `
        -t ("{0:F3}" -f $sourceDurationSeconds) `
        -movflags +faststart `
        $gridPath
    if ($LASTEXITCODE -ne 0) {
        throw "四镜头同步对照编码失败。"
    }

    $sequencePath = Join-Path $OutputRoot "米拉娜_四镜头顺序展示.mp4"
    $sequenceFilter = @"
[0:v]drawbox=x=0:y=0:w=iw:h=70:color=black@0.66:t=fill,drawtext=fontfile='$font':text='A  玩家视角':fontcolor=white:fontsize=34:x=28:y=17[v0];
[1:v]drawbox=x=0:y=0:w=iw:h=70:color=black@0.66:t=fill,drawtext=fontfile='$font':text='B  英雄跟随近景':fontcolor=white:fontsize=34:x=28:y=17[v1];
[2:v]drawbox=x=0:y=0:w=iw:h=70:color=black@0.66:t=fill,drawtext=fontfile='$font':text='C  高空俯视':fontcolor=white:fontsize=34:x=28:y=17[v2];
[3:v]drawbox=x=0:y=0:w=iw:h=70:color=black@0.66:t=fill,drawtext=fontfile='$font':text='D  推进跟移':fontcolor=white:fontsize=34:x=28:y=17[v3];
[0:a]anull[a0];
[1:a]anull[a1];
[2:a]anull[a2];
[3:a]anull[a3];
[v0][a0][v1][a1][v2][a2][v3][a3]concat=n=4:v=1:a=1[v][a]
"@ -replace "`r?`n", ""
    & $Ffmpeg -y `
        -i $rendered[0] -i $rendered[1] -i $rendered[2] -i $rendered[3] `
        -filter_complex $sequenceFilter `
        -map "[v]" -map "[a]" `
        -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p `
        -c:a aac -b:a 192k -ar 48000 `
        -movflags +faststart `
        $sequencePath
    if ($LASTEXITCODE -ne 0) {
        throw "四镜头顺序展示编码失败。"
    }

    $assets = foreach ($index in 0..($shots.Count - 1)) {
        $probe = & $Ffprobe -v error `
            -show_entries "format=duration:stream=codec_type,width,height,r_frame_rate" `
            -of json `
            $rendered[$index] | ConvertFrom-Json
        [ordered]@{
            assetId = $shots[$index].id
            label = $shots[$index].label
            purpose = $shots[$index].purpose
            outputPath = $rendered[$index]
            sourceStartSeconds = $sourceStartSeconds
            sourceEndSeconds = $sourceEndSeconds
            sourceStartTick = $sourceStartTick
            sourceEndTick = $sourceEndTick
            setupCommands = $shots[$index].setup
            cues = $shots[$index].cues
            durationSeconds = [double]$probe.format.duration
            width = [int]($probe.streams | Where-Object codec_type -eq "video").width
            height = [int]($probe.streams | Where-Object codec_type -eq "video").height
            hasAudio = [bool]($probe.streams | Where-Object codec_type -eq "audio")
        }
    }

    $manifest = [ordered]@{
        schemaVersion = "d2h.camera-showcase/1.0"
        matchId = "local-replay"
        protagonist = "npc_dota_hero_mirana"
        protagonistSlot = 9
        event = [ordered]@{
            timeSeconds = 1110.9667
            tick = 32670
            action = "mirana_arrow"
            target = "npc_dota_hero_ogre_magi"
            result = "hero_kill"
        }
        sourceWindow = [ordered]@{
            startSeconds = $sourceStartSeconds
            endSeconds = $sourceEndSeconds
            startTick = $sourceStartTick
            endTick = $sourceEndTick
        }
        assets = $assets
        comparisonVideos = @($gridPath, $sequencePath)
        limitation = "当前回放实际注册的 Dota 镜头接口可稳定控制观战模式、注视点和距离；低机位 pitch/yaw 命令未在本版本 Dota 回放相机中注册。"
    }
    $manifestPath = Join-Path $OutputRoot "镜头参数清单.json"
    $manifest | ConvertTo-Json -Depth 12 |
        Set-Content -LiteralPath $manifestPath -Encoding utf8

    $rawFiles = @(Get-ChildItem -LiteralPath $rawRoot -Recurse -File)
    foreach ($rawFile in $rawFiles) {
        [IO.File]::Delete($rawFile.FullName)
    }
    $rawDirectories = @(Get-ChildItem -LiteralPath $rawRoot -Recurse -Directory |
        Sort-Object { $_.FullName.Length } -Descending)
    foreach ($rawDirectory in $rawDirectories) {
        [IO.Directory]::Delete($rawDirectory.FullName)
    }
    [IO.Directory]::Delete($rawRoot)

    [ordered]@{
        outputRoot = $OutputRoot
        grid = $gridPath
        sequence = $sequencePath
        manifest = $manifestPath
        assets = $rendered
    } | ConvertTo-Json -Depth 6
}
finally {
    try {
        Invoke-VConsole @(
            "endmovie",
            "demo_pause",
            "dota_camera_distance 1200",
            "dota_camera_allow_freecam 0",
            "cl_drawhud 1",
            "dota_hide_cursor 0"
        ) | Out-Null
    }
    catch {
        Write-Warning "恢复镜头与 HUD 状态失败：$($_.Exception.Message)"
    }
}
