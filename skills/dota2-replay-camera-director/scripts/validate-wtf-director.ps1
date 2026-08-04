[CmdletBinding(DefaultParameterSetName = "Path")]
param(
    [Parameter(Mandatory = $true, ParameterSetName = "Path")]
    [string]$ProfilePath,

    [Parameter(ParameterSetName = "Path")]
    [string]$PlanPath,

    [Parameter(Mandatory = $true, ParameterSetName = "SelfTest")]
    [switch]$SelfTest
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Assert-Wtf {
    param([bool]$Condition, [string]$Message)
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

    Assert-Wtf ($null -ne $Object) "$Context must be an object."
    $names = @($Object.PSObject.Properties.Name)
    foreach ($name in $Required) {
        Assert-Wtf ($names -contains $name) "$Context is missing '$name'."
    }
    foreach ($name in $names) {
        Assert-Wtf ($Allowed -contains $name) "$Context contains unsupported field '$name'."
    }
}

function Assert-Text {
    param([object]$Value, [string]$Context)
    Assert-Wtf ($Value -is [string] -and -not [string]::IsNullOrWhiteSpace($Value)) `
        "$Context must be non-empty text."
}

function Get-WtfNumber {
    param([object]$Value, [string]$Context)
    Assert-Wtf ($null -ne $Value -and -not ($Value -is [string])) "$Context must be a number."
    try {
        $number = [double]$Value
    }
    catch {
        throw "$Context must be a number."
    }
    Assert-Wtf (-not [double]::IsNaN($number) -and -not [double]::IsInfinity($number)) `
        "$Context must be finite."
    return $number
}

function Get-WtfInteger {
    param([object]$Value, [string]$Context)
    $number = Get-WtfNumber $Value $Context
    Assert-Wtf ([Math]::Floor($number) -eq $number) "$Context must be an integer."
    return [long]$number
}

function Assert-TextArray {
    param(
        [object[]]$Values,
        [string]$Context,
        [string[]]$Allowed = @(),
        [bool]$AllowEmpty = $false
    )

    $items = @($Values)
    if (-not $AllowEmpty) {
        Assert-Wtf ($items.Count -gt 0) "$Context must not be empty."
    }
    $seen = @{}
    foreach ($item in $items) {
        Assert-Text $item $Context
        Assert-Wtf (-not $seen.ContainsKey($item)) "$Context contains '$item' twice."
        $seen[$item] = $true
        if ($Allowed.Count -gt 0) {
            Assert-Wtf ($Allowed -contains $item) "$Context contains unsupported value '$item'."
        }
    }
}

function Test-WtfProfileObject {
    param([object]$Document)

    Assert-Keys $Document `
        @(
            "schemaVersion", "profileId", "profileVersion", "status", "mode",
            "displayName", "sourceRefs", "runtimePolicy", "storyPatterns", "episodePolicy"
        ) `
        @(
            "schemaVersion", "profileId", "profileVersion", "status", "mode",
            "displayName", "sourceRefs", "runtimePolicy", "storyPatterns", "episodePolicy"
        ) `
        "profile"
    Assert-Wtf ($Document.schemaVersion -eq "d2h.wtf-director-profile/1.0") `
        "profile.schemaVersion is unsupported."
    Assert-Wtf ($Document.profileId -is [string] -and $Document.profileId -match "^[a-z0-9]+(?:-[a-z0-9]+)*$") `
        "profile.profileId must be a lowercase hyphen ID."
    Assert-Wtf ($Document.profileVersion -is [string] -and $Document.profileVersion -match "^\d+\.\d+\.\d+$") `
        "profile.profileVersion must be semantic version text."
    Assert-Wtf ($Document.status -in @("candidate", "validated")) "profile.status is unsupported."
    Assert-Wtf ($Document.mode -eq "wtf_director") "profile.mode must be wtf_director."
    Assert-Text $Document.displayName "profile.displayName"

    $sources = @($Document.sourceRefs)
    Assert-Wtf ($sources.Count -gt 0) "profile.sourceRefs must not be empty."
    if ($Document.status -eq "validated") {
        Assert-Wtf ($sources.Count -ge 2) "validated profile requires at least two sources."
    }
    $sourceIds = @{}
    foreach ($source in $sources) {
        Assert-Keys $source @("sourceId", "evidenceRef") @("sourceId", "evidenceRef") "sourceRef"
        Assert-Text $source.sourceId "sourceRef.sourceId"
        Assert-Text $source.evidenceRef "sourceRef.evidenceRef"
        Assert-Wtf (-not $sourceIds.ContainsKey($source.sourceId)) `
            "profile.sourceRefs duplicates '$($source.sourceId)'."
        $sourceIds[$source.sourceId] = $true
    }

    $policy = $Document.runtimePolicy
    Assert-Keys $policy `
        @(
            "defaultMode", "fallbackMode", "requiresCloudAi", "analysisStartsDota2",
            "preservePrecisionPlan", "allowUnverifiedPatterns", "externalAssetPolicy"
        ) `
        @(
            "defaultMode", "fallbackMode", "requiresCloudAi", "analysisStartsDota2",
            "preservePrecisionPlan", "allowUnverifiedPatterns", "externalAssetPolicy"
        ) `
        "runtimePolicy"
    Assert-Wtf ($policy.defaultMode -eq "precision") "runtimePolicy.defaultMode must be precision."
    Assert-Wtf ($policy.fallbackMode -eq "precision") "runtimePolicy.fallbackMode must be precision."
    Assert-Wtf ($policy.requiresCloudAi -is [bool] -and -not $policy.requiresCloudAi) `
        "runtimePolicy.requiresCloudAi must be false."
    Assert-Wtf ($policy.analysisStartsDota2 -is [bool] -and -not $policy.analysisStartsDota2) `
        "runtimePolicy.analysisStartsDota2 must be false."
    Assert-Wtf ($policy.preservePrecisionPlan -is [bool] -and $policy.preservePrecisionPlan) `
        "runtimePolicy.preservePrecisionPlan must be true."
    Assert-Wtf ($policy.allowUnverifiedPatterns -is [bool]) `
        "runtimePolicy.allowUnverifiedPatterns must be boolean."
    Assert-Wtf ($policy.externalAssetPolicy -in @("semantic_slots_only", "local_licensed_library")) `
        "runtimePolicy.externalAssetPolicy is unsupported."

    $allowedCategories = @("comedy", "skill", "mistake", "fight", "objective")
    $allowedEvidence = @("trigger", "response", "verification", "kill", "reaction", "context")
    $allowedRoles = @(
        "setup", "recognition", "development", "turn", "technical_proof",
        "reaction", "payoff", "result"
    )
    $allowedSelectors = @(
        "first", "second", "middle_representative", "highest_salience", "last", "all"
    )
    $allowedCueTypes = @("reaction_slot", "callback", "mechanic_card", "impact_hold", "clean_replay")
    $allowedExecution = @("automatic_clean_edit", "marker_only")
    $allowedCameras = @("player-view", "hero-chase-close", "high-aerial", "push-track")

    $patterns = @($Document.storyPatterns)
    Assert-Wtf ($patterns.Count -gt 0) "profile.storyPatterns must not be empty."
    $patternIds = @{}
    $allPatternsValidated = $true
    foreach ($pattern in $patterns) {
        Assert-Keys $pattern `
            @(
                "patternId", "title", "validationState", "categories", "signal",
                "beatProgram", "jokePointProgram", "coverageProgram", "selectionPolicy"
            ) `
            @(
                "patternId", "title", "validationState", "categories", "signal",
                "beatProgram", "jokePointProgram", "coverageProgram", "selectionPolicy"
            ) `
            "storyPattern"
        Assert-Wtf ($pattern.patternId -is [string] -and $pattern.patternId -match "^[a-z0-9]+(?:-[a-z0-9]+)*$") `
            "storyPattern.patternId must be a lowercase hyphen ID."
        Assert-Wtf (-not $patternIds.ContainsKey($pattern.patternId)) `
            "profile.storyPatterns duplicates '$($pattern.patternId)'."
        $patternIds[$pattern.patternId] = $pattern
        Assert-Text $pattern.title "storyPattern.title"
        Assert-Wtf ($pattern.validationState -in @("pilot_only", "validated")) `
            "storyPattern.validationState is unsupported."
        if ($pattern.validationState -ne "validated") {
            $allPatternsValidated = $false
        }
        Assert-TextArray @($pattern.categories) "storyPattern.categories" $allowedCategories

        $signal = $pattern.signal
        Assert-Keys $signal `
            @("evidenceKinds", "groupingKeys", "minOccurrences", "maxSpanSeconds", "minimumConfidence") `
            @("evidenceKinds", "groupingKeys", "minOccurrences", "maxSpanSeconds", "minimumConfidence") `
            "storyPattern.signal"
        Assert-TextArray @($signal.evidenceKinds) "storyPattern.signal.evidenceKinds" $allowedEvidence
        Assert-TextArray @($signal.groupingKeys) "storyPattern.signal.groupingKeys"
        $minOccurrences = Get-WtfInteger $signal.minOccurrences "storyPattern.signal.minOccurrences"
        Assert-Wtf ($minOccurrences -ge 1 -and $minOccurrences -le 20) `
            "storyPattern.signal.minOccurrences must be inside 1..20."
        $maxSpan = Get-WtfNumber $signal.maxSpanSeconds "storyPattern.signal.maxSpanSeconds"
        Assert-Wtf ($maxSpan -gt 0 -and $maxSpan -le 7200) `
            "storyPattern.signal.maxSpanSeconds must be inside 0..7200."
        $minimumConfidence = Get-WtfNumber $signal.minimumConfidence "storyPattern.signal.minimumConfidence"
        Assert-Wtf ($minimumConfidence -ge 0 -and $minimumConfidence -le 1) `
            "storyPattern.signal.minimumConfidence must be inside 0..1."

        $beats = @($pattern.beatProgram)
        Assert-Wtf ($beats.Count -ge 2) "storyPattern.beatProgram requires at least two beats."
        $beatIds = @{}
        $beatRoles = @()
        foreach ($beat in $beats) {
            Assert-Keys $beat `
                @("beatId", "role", "selector", "required", "purpose") `
                @("beatId", "role", "selector", "required", "purpose") `
                "beatProgram"
            Assert-Wtf ($beat.beatId -is [string] -and $beat.beatId -match "^[a-z0-9]+(?:-[a-z0-9]+)*$") `
                "beatProgram.beatId must be a lowercase hyphen ID."
            Assert-Wtf (-not $beatIds.ContainsKey($beat.beatId)) `
                "beatProgram duplicates '$($beat.beatId)'."
            $beatIds[$beat.beatId] = $beat
            Assert-Wtf ($allowedRoles -contains $beat.role) "beatProgram.role is unsupported."
            Assert-Wtf ($allowedSelectors -contains $beat.selector) "beatProgram.selector is unsupported."
            Assert-Wtf ($beat.required -is [bool]) "beatProgram.required must be boolean."
            Assert-Text $beat.purpose "beatProgram.purpose"
            $beatRoles += $beat.role
        }
        Assert-Wtf ($beatRoles -contains "setup") "storyPattern requires a setup beat."
        Assert-Wtf (@($beatRoles | Where-Object { $_ -in @("payoff", "result") }).Count -gt 0) `
            "storyPattern requires a payoff or result beat."

        $programCues = @{}
        foreach ($cue in @($pattern.jokePointProgram)) {
            Assert-Keys $cue `
                @("cueId", "anchorBeatId", "cueType", "execution", "purpose") `
                @("cueId", "anchorBeatId", "cueType", "execution", "purpose") `
                "jokePointProgram"
            Assert-Wtf ($cue.cueId -is [string] -and $cue.cueId -match "^[a-z0-9]+(?:-[a-z0-9]+)*$") `
                "jokePointProgram.cueId must be a lowercase hyphen ID."
            Assert-Wtf (-not $programCues.ContainsKey($cue.cueId)) `
                "jokePointProgram duplicates '$($cue.cueId)'."
            Assert-Wtf ($beatIds.ContainsKey($cue.anchorBeatId)) `
                "jokePointProgram references unknown beat '$($cue.anchorBeatId)'."
            Assert-Wtf ($allowedCueTypes -contains $cue.cueType) "jokePointProgram.cueType is unsupported."
            Assert-Wtf ($allowedExecution -contains $cue.execution) `
                "jokePointProgram.execution is unsupported."
            Assert-Text $cue.purpose "jokePointProgram.purpose"
            $programCues[$cue.cueId] = $cue
        }

        $coverageBeats = @{}
        foreach ($coverage in @($pattern.coverageProgram)) {
            Assert-Keys $coverage `
                @("beatId", "primaryCamera", "alternateCameras", "alternateWhen", "purpose") `
                @("beatId", "primaryCamera", "alternateCameras", "alternateWhen", "purpose") `
                "coverageProgram"
            Assert-Wtf ($beatIds.ContainsKey($coverage.beatId)) `
                "coverageProgram references unknown beat '$($coverage.beatId)'."
            Assert-Wtf (-not $coverageBeats.ContainsKey($coverage.beatId)) `
                "coverageProgram duplicates beat '$($coverage.beatId)'."
            $coverageBeats[$coverage.beatId] = $true
            Assert-Wtf ($coverage.primaryCamera -eq "player-view") `
                "coverageProgram.primaryCamera must be player-view."
            Assert-TextArray @($coverage.alternateCameras) "coverageProgram.alternateCameras" `
                $allowedCameras $true
            Assert-Wtf (-not (@($coverage.alternateCameras) -contains "player-view")) `
                "coverageProgram.alternateCameras must not repeat player-view."
            Assert-TextArray @($coverage.alternateWhen) "coverageProgram.alternateWhen" @() $true
            Assert-Text $coverage.purpose "coverageProgram.purpose"
        }
        foreach ($requiredBeat in @($beats | Where-Object { $_.required })) {
            Assert-Wtf ($coverageBeats.ContainsKey($requiredBeat.beatId)) `
                "required beat '$($requiredBeat.beatId)' has no coverage rule."
        }

        $selection = $pattern.selectionPolicy
        Assert-Keys $selection `
            @("maxOccurrencesInCut", "preserveFirst", "preserveLast", "middleSelector") `
            @("maxOccurrencesInCut", "preserveFirst", "preserveLast", "middleSelector") `
            "selectionPolicy"
        $maxOccurrences = Get-WtfInteger $selection.maxOccurrencesInCut `
            "selectionPolicy.maxOccurrencesInCut"
        Assert-Wtf ($maxOccurrences -ge 1 -and $maxOccurrences -le 20) `
            "selectionPolicy.maxOccurrencesInCut must be inside 1..20."
        Assert-Wtf ($selection.preserveFirst -is [bool]) "selectionPolicy.preserveFirst must be boolean."
        Assert-Wtf ($selection.preserveLast -is [bool]) "selectionPolicy.preserveLast must be boolean."
        Assert-Wtf ($selection.middleSelector -in @("highest_information_gain", "highest_salience", "chronological")) `
            "selectionPolicy.middleSelector is unsupported."
    }

    $episode = $Document.episodePolicy
    Assert-Keys $episode `
        @("targetDurationSeconds", "maxStories", "orderingSignals", "finalePreference") `
        @("targetDurationSeconds", "maxStories", "orderingSignals", "finalePreference") `
        "episodePolicy"
    $targetDuration = Get-WtfNumber $episode.targetDurationSeconds "episodePolicy.targetDurationSeconds"
    Assert-Wtf ($targetDuration -ge 15 -and $targetDuration -le 1800) `
        "episodePolicy.targetDurationSeconds must be inside 15..1800."
    $maxStories = Get-WtfInteger $episode.maxStories "episodePolicy.maxStories"
    Assert-Wtf ($maxStories -ge 1 -and $maxStories -le 20) `
        "episodePolicy.maxStories must be inside 1..20."
    Assert-TextArray @($episode.orderingSignals) "episodePolicy.orderingSignals"
    Assert-TextArray @($episode.finalePreference) "episodePolicy.finalePreference"

    $runtimeEligible = (
        $Document.status -eq "validated" -and
        $sources.Count -ge 2 -and
        $allPatternsValidated -and
        -not $policy.allowUnverifiedPatterns
    )

    return [ordered]@{
        valid = $true
        schemaVersion = $Document.schemaVersion
        profileId = $Document.profileId
        profileVersion = $Document.profileVersion
        patternCount = $patterns.Count
        runtimeEligible = $runtimeEligible
    }
}

function Test-WtfPlanObject {
    param([object]$Document, [object]$Profile, [object]$ProfileSummary)

    Assert-Wtf $ProfileSummary.runtimeEligible `
        "plan cannot use a profile that is not runtime eligible."
    Assert-Keys $Document `
        @(
            "schemaVersion", "mode", "profileRef", "sourceSha256", "fallbackMode",
            "precisionPlanRef", "protagonist", "stories", "cameraPlanRef", "roughCut",
            "reviewRequired"
        ) `
        @(
            "schemaVersion", "mode", "profileRef", "sourceSha256", "fallbackMode",
            "precisionPlanRef", "protagonist", "stories", "cameraPlanRef", "roughCut",
            "reviewRequired"
        ) `
        "plan"
    Assert-Wtf ($Document.schemaVersion -eq "d2h.wtf-director-plan/1.0") `
        "plan.schemaVersion is unsupported."
    Assert-Wtf ($Document.mode -eq "wtf_director") "plan.mode must be wtf_director."
    Assert-Wtf ($Document.fallbackMode -eq "precision") "plan.fallbackMode must be precision."
    Assert-Wtf ($Document.precisionPlanRef -eq "director/edit-plan.json") `
        "plan.precisionPlanRef must preserve the existing precision plan."
    Assert-Wtf ($Document.sourceSha256 -is [string] -and $Document.sourceSha256 -match "^[0-9a-fA-F]{64}$") `
        "plan.sourceSha256 must be a SHA-256 hex string."
    Assert-Text $Document.cameraPlanRef "plan.cameraPlanRef"
    Assert-Wtf ($Document.reviewRequired -is [bool]) "plan.reviewRequired must be boolean."

    Assert-Keys $Document.profileRef @("profileId", "profileVersion") `
        @("profileId", "profileVersion") "plan.profileRef"
    Assert-Wtf ($Document.profileRef.profileId -eq $Profile.profileId) `
        "plan.profileRef.profileId does not match the profile."
    Assert-Wtf ($Document.profileRef.profileVersion -eq $Profile.profileVersion) `
        "plan.profileRef.profileVersion does not match the profile."

    Assert-Keys $Document.protagonist @("hero", "slot") @("hero", "slot") "plan.protagonist"
    Assert-Text $Document.protagonist.hero "plan.protagonist.hero"
    $slot = Get-WtfInteger $Document.protagonist.slot "plan.protagonist.slot"
    Assert-Wtf ($slot -ge 0 -and $slot -le 9) "plan.protagonist.slot must be inside 0..9."

    $patternMap = @{}
    foreach ($pattern in @($Profile.storyPatterns)) {
        $patternMap[$pattern.patternId] = $pattern
    }
    $stories = @($Document.stories)
    Assert-Wtf ($stories.Count -gt 0) "plan.stories must not be empty."
    $storyIds = @{}
    $allSceneIds = @{}
    foreach ($story in $stories) {
        Assert-Keys $story `
            @(
                "storyId", "patternId", "title", "confidence", "evidenceIds",
                "beats", "jokePoints", "sceneIds"
            ) `
            @(
                "storyId", "patternId", "title", "confidence", "evidenceIds",
                "beats", "jokePoints", "sceneIds"
            ) `
            "story"
        Assert-Wtf ($story.storyId -is [string] -and $story.storyId -match "^W\d{3}$") `
            "story.storyId must match W001."
        Assert-Wtf (-not $storyIds.ContainsKey($story.storyId)) `
            "plan.stories duplicates '$($story.storyId)'."
        $storyIds[$story.storyId] = $true
        Assert-Wtf ($patternMap.ContainsKey($story.patternId)) `
            "story.patternId '$($story.patternId)' is unknown."
        $pattern = $patternMap[$story.patternId]
        Assert-Text $story.title "story.title"

        Assert-Keys $story.confidence @("score", "reasons") @("score", "reasons") `
            "story.confidence"
        $score = Get-WtfNumber $story.confidence.score "story.confidence.score"
        Assert-Wtf ($score -ge 0 -and $score -le 1) "story.confidence.score must be inside 0..1."
        Assert-TextArray @($story.confidence.reasons) "story.confidence.reasons"
        Assert-TextArray @($story.evidenceIds) "story.evidenceIds"
        $storyEvidence = @{}
        foreach ($evidenceId in @($story.evidenceIds)) {
            $storyEvidence[$evidenceId] = $true
        }

        $allowedPatternRoles = @($pattern.beatProgram | ForEach-Object { $_.role })
        $beats = @($story.beats)
        Assert-Wtf ($beats.Count -gt 0) "story.beats must not be empty."
        $runtimeBeats = @{}
        $storyStart = [double]::PositiveInfinity
        $storyEnd = 0.0
        foreach ($beat in $beats) {
            Assert-Keys $beat `
                @("beatId", "role", "sourceStartSeconds", "sourceEndSeconds", "evidenceIds") `
                @("beatId", "role", "sourceStartSeconds", "sourceEndSeconds", "evidenceIds") `
                "story.beat"
            Assert-Wtf ($beat.beatId -is [string] -and $beat.beatId -match "^W\d{3}-B\d{2}$") `
                "story.beat.beatId must match W001-B01."
            Assert-Wtf (-not $runtimeBeats.ContainsKey($beat.beatId)) `
                "story.beats duplicates '$($beat.beatId)'."
            $runtimeBeats[$beat.beatId] = $beat
            Assert-Wtf ($allowedPatternRoles -contains $beat.role) `
                "story.beat.role '$($beat.role)' is not defined by the pattern."
            $start = Get-WtfNumber $beat.sourceStartSeconds "story.beat.sourceStartSeconds"
            $end = Get-WtfNumber $beat.sourceEndSeconds "story.beat.sourceEndSeconds"
            Assert-Wtf ($start -ge 0 -and $end -gt $start -and ($end - $start) -le 90) `
                "story.beat source window is invalid."
            if ($start -lt $storyStart) { $storyStart = $start }
            if ($end -gt $storyEnd) { $storyEnd = $end }
            Assert-TextArray @($beat.evidenceIds) "story.beat.evidenceIds"
            foreach ($evidenceId in @($beat.evidenceIds)) {
                Assert-Wtf ($storyEvidence.ContainsKey($evidenceId)) `
                    "story.beat references unknown evidence '$evidenceId'."
            }
        }

        $cueMap = @{}
        foreach ($cue in @($pattern.jokePointProgram)) {
            $cueMap[$cue.cueId] = $cue
        }
        $runtimeCueIds = @{}
        foreach ($jokePoint in @($story.jokePoints)) {
            Assert-Keys $jokePoint `
                @("cueId", "programCueId", "anchorSeconds", "cueType", "execution", "purpose") `
                @("cueId", "programCueId", "anchorSeconds", "cueType", "execution", "purpose") `
                "story.jokePoint"
            Assert-Wtf ($jokePoint.cueId -is [string] -and $jokePoint.cueId -match "^W\d{3}-J\d{2}$") `
                "story.jokePoint.cueId must match W001-J01."
            Assert-Wtf (-not $runtimeCueIds.ContainsKey($jokePoint.cueId)) `
                "story.jokePoints duplicates '$($jokePoint.cueId)'."
            $runtimeCueIds[$jokePoint.cueId] = $true
            Assert-Wtf ($cueMap.ContainsKey($jokePoint.programCueId)) `
                "story.jokePoint references unknown program cue '$($jokePoint.programCueId)'."
            $programCue = $cueMap[$jokePoint.programCueId]
            Assert-Wtf ($jokePoint.cueType -eq $programCue.cueType) `
                "story.jokePoint.cueType differs from the profile."
            Assert-Wtf ($jokePoint.execution -eq $programCue.execution) `
                "story.jokePoint.execution differs from the profile."
            $anchor = Get-WtfNumber $jokePoint.anchorSeconds "story.jokePoint.anchorSeconds"
            Assert-Wtf ($anchor -ge $storyStart -and $anchor -le $storyEnd) `
                "story.jokePoint.anchorSeconds is outside the story."
            Assert-Text $jokePoint.purpose "story.jokePoint.purpose"
        }

        $sceneIds = @($story.sceneIds)
        Assert-TextArray $sceneIds "story.sceneIds"
        foreach ($sceneId in $sceneIds) {
            Assert-Wtf ($sceneId -match "^S\d{3}$") "story.sceneIds must match S001."
            Assert-Wtf (-not $allSceneIds.ContainsKey($sceneId)) `
                "plan stories reuse scene '$sceneId'."
            $allSceneIds[$sceneId] = $story.storyId
        }
    }

    $roughCut = @($Document.roughCut)
    Assert-Wtf ($roughCut.Count -gt 0) "plan.roughCut must not be empty."
    $roughScenes = @{}
    for ($index = 0; $index -lt $roughCut.Count; $index++) {
        $item = $roughCut[$index]
        Assert-Keys $item `
            @("order", "sceneId", "takeId", "sourceStartSeconds", "sourceEndSeconds") `
            @("order", "sceneId", "takeId", "sourceStartSeconds", "sourceEndSeconds") `
            "roughCut"
        $order = Get-WtfInteger $item.order "roughCut.order"
        Assert-Wtf ($order -eq ($index + 1)) "roughCut.order must be contiguous from 1."
        Assert-Wtf ($allSceneIds.ContainsKey($item.sceneId)) `
            "roughCut references unknown scene '$($item.sceneId)'."
        Assert-Wtf (-not $roughScenes.ContainsKey($item.sceneId)) `
            "roughCut contains scene '$($item.sceneId)' twice."
        $roughScenes[$item.sceneId] = $true
        Assert-Wtf ($item.takeId -is [string] -and $item.takeId -match "^$([regex]::Escape($item.sceneId))-[A-H]$") `
            "roughCut.takeId must belong to its scene."
        $start = Get-WtfNumber $item.sourceStartSeconds "roughCut.sourceStartSeconds"
        $end = Get-WtfNumber $item.sourceEndSeconds "roughCut.sourceEndSeconds"
        Assert-Wtf ($start -ge 0 -and $end -gt $start -and ($end - $start) -le 90) `
            "roughCut source window is invalid."
    }

    return [ordered]@{
        valid = $true
        schemaVersion = $Document.schemaVersion
        storyCount = $stories.Count
        roughCutCount = $roughCut.Count
    }
}

$skillRoot = Split-Path -Parent $PSScriptRoot
$exampleProfilePath = Join-Path $skillRoot "references\wtf-director-profile-example.json"
$examplePlanPath = Join-Path $skillRoot "references\wtf-director-plan-example.json"

if ($SelfTest) {
    $profile = Get-Content -Raw -Encoding UTF8 -LiteralPath $exampleProfilePath | ConvertFrom-Json
    $plan = Get-Content -Raw -Encoding UTF8 -LiteralPath $examplePlanPath | ConvertFrom-Json
    $profileSummary = Test-WtfProfileObject $profile
    $planSummary = Test-WtfPlanObject $plan $profile $profileSummary

    $invalidPlan = $plan | ConvertTo-Json -Depth 30 | ConvertFrom-Json
    $invalidPlan.mode = "precision"
    $rejectedWrongMode = $false
    try {
        Test-WtfPlanObject $invalidPlan $profile $profileSummary | Out-Null
    }
    catch {
        $rejectedWrongMode = $true
    }
    Assert-Wtf $rejectedWrongMode "Self-test did not reject a precision plan."

    $candidateProfile = $profile | ConvertTo-Json -Depth 30 | ConvertFrom-Json
    $candidateProfile.status = "candidate"
    $candidateProfile.storyPatterns[0].validationState = "pilot_only"
    $candidateSummary = Test-WtfProfileObject $candidateProfile
    Assert-Wtf (-not $candidateSummary.runtimeEligible) `
        "Self-test candidate profile unexpectedly became runtime eligible."

    $rejectedCandidateRuntime = $false
    try {
        Test-WtfPlanObject $plan $candidateProfile $candidateSummary | Out-Null
    }
    catch {
        $rejectedCandidateRuntime = $true
    }
    Assert-Wtf $rejectedCandidateRuntime "Self-test did not reject a candidate runtime profile."

    [ordered]@{
        validProfile = $profileSummary.valid
        validPlan = $planSummary.valid
        rejectedWrongMode = $rejectedWrongMode
        candidateRuntimeEligible = $candidateSummary.runtimeEligible
        rejectedCandidateRuntime = $rejectedCandidateRuntime
        patterns = $profileSummary.patternCount
        stories = $planSummary.storyCount
    } | ConvertTo-Json
    exit 0
}

$resolvedProfilePath = Resolve-Path -LiteralPath $ProfilePath -ErrorAction Stop
$profile = Get-Content -Raw -Encoding UTF8 -LiteralPath $resolvedProfilePath | ConvertFrom-Json
$profileSummary = Test-WtfProfileObject $profile

if ([string]::IsNullOrWhiteSpace($PlanPath)) {
    $profileSummary | ConvertTo-Json
    exit 0
}

$resolvedPlanPath = Resolve-Path -LiteralPath $PlanPath -ErrorAction Stop
$plan = Get-Content -Raw -Encoding UTF8 -LiteralPath $resolvedPlanPath | ConvertFrom-Json
[ordered]@{
    profile = $profileSummary
    plan = Test-WtfPlanObject $plan $profile $profileSummary
} | ConvertTo-Json -Depth 5
