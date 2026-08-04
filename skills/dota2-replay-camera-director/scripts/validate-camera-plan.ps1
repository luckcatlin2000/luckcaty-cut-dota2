[CmdletBinding(DefaultParameterSetName = "Path")]
param(
    [Parameter(Mandatory = $true, ParameterSetName = "Path")]
    [string]$Path,

    [Parameter(Mandatory = $true, ParameterSetName = "SelfTest")]
    [switch]$SelfTest
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Assert-Plan {
    param(
        [bool]$Condition,
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Keys {
    param(
        [object]$Object,
        [string[]]$Required,
        [string[]]$Allowed,
        [string]$Context
    )

    Assert-Plan ($null -ne $Object) "$Context must be an object."
    $names = @($Object.PSObject.Properties.Name)
    foreach ($name in $Required) {
        Assert-Plan ($names -contains $name) "$Context is missing '$name'."
    }
    foreach ($name in $names) {
        Assert-Plan ($Allowed -contains $name) "$Context contains unsupported field '$name'."
    }
}

function Get-PlanNumber {
    param(
        [object]$Value,
        [string]$Context
    )

    Assert-Plan ($null -ne $Value -and -not ($Value -is [string])) "$Context must be a number."
    try {
        $number = [double]$Value
    }
    catch {
        throw "$Context must be a number."
    }
    Assert-Plan (-not [double]::IsNaN($number) -and -not [double]::IsInfinity($number)) `
        "$Context must be finite."
    return $number
}

function Get-PlanInteger {
    param(
        [object]$Value,
        [string]$Context
    )

    $number = Get-PlanNumber $Value $Context
    Assert-Plan ([Math]::Floor($number) -eq $number) "$Context must be an integer."
    return [long]$number
}

function Assert-Text {
    param(
        [object]$Value,
        [string]$Context
    )

    Assert-Plan ($Value -is [string] -and -not [string]::IsNullOrWhiteSpace($Value)) `
        "$Context must be non-empty text."
}

function Assert-LookAt {
    param(
        [object]$LookAt,
        [string]$Context
    )

    Assert-Keys $LookAt @("x", "y") @("x", "y") $Context
    $x = Get-PlanNumber $LookAt.x "$Context.x"
    $y = Get-PlanNumber $LookAt.y "$Context.y"
    Assert-Plan ($x -ge -10000 -and $x -le 10000) "$Context.x is outside the calibrated boundary."
    Assert-Plan ($y -ge -10000 -and $y -le 10000) "$Context.y is outside the calibrated boundary."
}

function Test-CameraPlanObject {
    param([object]$Document)

    Assert-Keys $Document `
        @("schemaVersion", "replayRef", "protagonist", "scenes") `
        @("schemaVersion", "replayRef", "protagonist", "scenes") `
        "plan"
    Assert-Plan ($Document.schemaVersion -eq "d2h.camera-skill-plan/1.0") `
        "plan.schemaVersion is unsupported."
    Assert-Text $Document.replayRef "plan.replayRef"

    Assert-Keys $Document.protagonist @("hero", "slot") @("hero", "slot") "plan.protagonist"
    Assert-Text $Document.protagonist.hero "plan.protagonist.hero"
    $protagonistSlot = Get-PlanInteger $Document.protagonist.slot "plan.protagonist.slot"
    Assert-Plan ($protagonistSlot -ge 0 -and $protagonistSlot -le 9) `
        "plan.protagonist.slot must be between 0 and 9."

    $scenes = @($Document.scenes)
    Assert-Plan ($scenes.Count -gt 0) "plan.scenes must contain at least one scene."
    $seenScenes = @{}

    foreach ($scene in $scenes) {
        Assert-Keys $scene `
            @(
                "sceneId",
                "candidateId",
                "storyPurpose",
                "evidenceIds",
                "source",
                "takes",
                "suggestedSwitchWindows",
                "fallbackTakeId"
            ) `
            @(
                "sceneId",
                "candidateId",
                "storyPurpose",
                "evidenceIds",
                "source",
                "takes",
                "suggestedSwitchWindows",
                "fallbackTakeId"
            ) `
            "scene"
        Assert-Plan ($scene.sceneId -is [string] -and $scene.sceneId -match "^S\d{3}$") `
            "scene.sceneId must match S001."
        Assert-Plan (-not $seenScenes.ContainsKey($scene.sceneId)) `
            "scene.sceneId '$($scene.sceneId)' is duplicated."
        $seenScenes[$scene.sceneId] = $true
        Assert-Text $scene.candidateId "$($scene.sceneId).candidateId"
        Assert-Text $scene.storyPurpose "$($scene.sceneId).storyPurpose"

        $evidenceIds = @($scene.evidenceIds)
        Assert-Plan ($evidenceIds.Count -gt 0) "$($scene.sceneId).evidenceIds must not be empty."
        $seenEvidence = @{}
        foreach ($evidenceId in $evidenceIds) {
            Assert-Text $evidenceId "$($scene.sceneId).evidenceIds"
            Assert-Plan (-not $seenEvidence.ContainsKey($evidenceId)) `
                "$($scene.sceneId).evidenceIds contains '$evidenceId' twice."
            $seenEvidence[$evidenceId] = $true
        }

        Assert-Keys $scene.source `
            @("startSeconds", "endSeconds", "startTick", "endTick") `
            @("startSeconds", "endSeconds", "startTick", "endTick") `
            "$($scene.sceneId).source"
        $startSeconds = Get-PlanNumber $scene.source.startSeconds "$($scene.sceneId).source.startSeconds"
        $endSeconds = Get-PlanNumber $scene.source.endSeconds "$($scene.sceneId).source.endSeconds"
        $startTick = Get-PlanInteger $scene.source.startTick "$($scene.sceneId).source.startTick"
        $endTick = Get-PlanInteger $scene.source.endTick "$($scene.sceneId).source.endTick"
        $duration = $endSeconds - $startSeconds
        Assert-Plan ($startSeconds -ge 0) "$($scene.sceneId).source.startSeconds must be non-negative."
        Assert-Plan ($duration -ge 1 -and $duration -le 90) `
            "$($scene.sceneId) duration must be between 1 and 90 seconds."
        Assert-Plan ($startTick -ge 0 -and $endTick -gt $startTick) `
            "$($scene.sceneId) tick range is invalid."

        $takes = @($scene.takes)
        Assert-Plan ($takes.Count -ge 1 -and $takes.Count -le 8) `
            "$($scene.sceneId).takes must contain 1 to 8 takes."
        $takeIds = @{}
        $primaryTakes = @()
        $allowedCameraTypes = @(
            "player-view",
            "hero-chase-close",
            "high-aerial",
            "push-track"
        )

        for ($takeIndex = 0; $takeIndex -lt $takes.Count; $takeIndex++) {
            $take = $takes[$takeIndex]
            $context = "$($scene.sceneId).takes[$takeIndex]"
            Assert-Keys $take `
                @("takeId", "cameraType", "primary", "distance") `
                @("takeId", "cameraType", "primary", "targetSlot", "distance", "lookAt", "cues") `
                $context
            $expectedTakeId = "{0}-{1}" -f $scene.sceneId, [char](65 + $takeIndex)
            Assert-Plan ($take.takeId -eq $expectedTakeId) `
                "$context.takeId must be '$expectedTakeId'."
            Assert-Plan (-not $takeIds.ContainsKey($take.takeId)) `
                "$context.takeId is duplicated."
            $takeIds[$take.takeId] = $take
            Assert-Plan ($allowedCameraTypes -contains $take.cameraType) `
                "$context.cameraType is unsupported."
            Assert-Plan ($take.primary -is [bool]) "$context.primary must be boolean."
            if ($take.primary) {
                $primaryTakes += $take
            }

            $distance = Get-PlanNumber $take.distance "$context.distance"
            Assert-Plan ($distance -ge 400 -and $distance -le 3000) `
                "$context.distance is outside 400..3000."

            if ($take.cameraType -in @("player-view", "hero-chase-close")) {
                Assert-Plan ($take.PSObject.Properties.Name -contains "targetSlot") `
                    "$context.targetSlot is required."
                $targetSlot = Get-PlanInteger $take.targetSlot "$context.targetSlot"
                Assert-Plan ($targetSlot -ge 0 -and $targetSlot -le 9) `
                    "$context.targetSlot must be between 0 and 9."
            }

            if ($take.cameraType -in @("high-aerial", "push-track")) {
                Assert-Plan ($take.PSObject.Properties.Name -contains "lookAt") `
                    "$context.lookAt is required."
                Assert-LookAt $take.lookAt "$context.lookAt"
            }

            $cues = @()
            if ($take.PSObject.Properties.Name -contains "cues") {
                $cues = @($take.cues)
            }
            if ($take.cameraType -eq "push-track") {
                Assert-Plan ($cues.Count -gt 0) "$context.cues must not be empty."
            }
            else {
                Assert-Plan ($cues.Count -eq 0) "$context.cues are only valid for push-track."
            }

            $previousCue = 0.0
            for ($cueIndex = 0; $cueIndex -lt $cues.Count; $cueIndex++) {
                $cue = $cues[$cueIndex]
                $cueContext = "$context.cues[$cueIndex]"
                Assert-Keys $cue `
                    @("atSeconds") `
                    @("atSeconds", "distance", "lookAt") `
                    $cueContext
                $atSeconds = Get-PlanNumber $cue.atSeconds "$cueContext.atSeconds"
                Assert-Plan ($atSeconds -gt $previousCue -and $atSeconds -lt $duration) `
                    "$cueContext.atSeconds must increase and stay inside the scene."
                $previousCue = $atSeconds
                $hasDistance = $cue.PSObject.Properties.Name -contains "distance"
                $hasLookAt = $cue.PSObject.Properties.Name -contains "lookAt"
                Assert-Plan ($hasDistance -or $hasLookAt) `
                    "$cueContext must change distance or lookAt."
                if ($hasDistance) {
                    $cueDistance = Get-PlanNumber $cue.distance "$cueContext.distance"
                    Assert-Plan ($cueDistance -ge 400 -and $cueDistance -le 3000) `
                        "$cueContext.distance is outside 400..3000."
                }
                if ($hasLookAt) {
                    Assert-LookAt $cue.lookAt "$cueContext.lookAt"
                }
            }
        }

        Assert-Plan ($primaryTakes.Count -eq 1) `
            "$($scene.sceneId) must contain exactly one primary take."
        Assert-Plan (
            $primaryTakes[0].takeId -eq "$($scene.sceneId)-A" -and
            $primaryTakes[0].cameraType -eq "player-view"
        ) "$($scene.sceneId)-A must be the primary player-view take."
        Assert-Plan ($scene.fallbackTakeId -eq $primaryTakes[0].takeId) `
            "$($scene.sceneId).fallbackTakeId must point to the primary take."

        $switchWindows = @($scene.suggestedSwitchWindows)
        foreach ($window in $switchWindows) {
            $context = "$($scene.sceneId).suggestedSwitchWindows"
            Assert-Keys $window `
                @("takeId", "startOffsetSeconds", "endOffsetSeconds", "reason") `
                @("takeId", "startOffsetSeconds", "endOffsetSeconds", "reason") `
                $context
            Assert-Plan ($takeIds.ContainsKey($window.takeId)) `
                "$context references unknown take '$($window.takeId)'."
            Assert-Plan (-not $takeIds[$window.takeId].primary) `
                "$context must reference an alternate take."
            $switchStart = Get-PlanNumber $window.startOffsetSeconds "$context.startOffsetSeconds"
            $switchEnd = Get-PlanNumber $window.endOffsetSeconds "$context.endOffsetSeconds"
            Assert-Plan (
                $switchStart -ge 0 -and
                $switchEnd -gt $switchStart -and
                $switchEnd -le $duration
            ) "$context is outside the scene duration."
            Assert-Text $window.reason "$context.reason"
        }
    }

    return [ordered]@{
        valid = $true
        schemaVersion = $Document.schemaVersion
        sceneCount = $scenes.Count
        takeCount = @($scenes | ForEach-Object { @($_.takes).Count } | Measure-Object -Sum).Sum
    }
}

$skillRoot = Split-Path -Parent $PSScriptRoot
$examplePath = Join-Path $skillRoot "references\mirana-arrow-example.json"

if ($SelfTest) {
    $example = Get-Content -Raw -Encoding UTF8 -LiteralPath $examplePath | ConvertFrom-Json
    $summary = Test-CameraPlanObject $example

    $invalid = $example | ConvertTo-Json -Depth 20 | ConvertFrom-Json
    $invalid.scenes[0].takes[1].takeId = "S999-Z"
    $rejectedInvalidPlan = $false
    try {
        Test-CameraPlanObject $invalid | Out-Null
    }
    catch {
        $rejectedInvalidPlan = $true
    }
    Assert-Plan $rejectedInvalidPlan "Self-test did not reject an invalid take ID."

    [ordered]@{
        validExample = $summary.valid
        rejectedInvalidPlan = $rejectedInvalidPlan
        scenes = $summary.sceneCount
        takes = $summary.takeCount
    } | ConvertTo-Json
    exit 0
}

$resolvedPath = Resolve-Path -LiteralPath $Path -ErrorAction Stop
$document = Get-Content -Raw -Encoding UTF8 -LiteralPath $resolvedPath | ConvertFrom-Json
Test-CameraPlanObject $document | ConvertTo-Json
