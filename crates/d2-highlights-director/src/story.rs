use d2_highlights_core::{
    HeroKillMoment, HighlightCandidate, HighlightDocument, HighlightStory, STORY_SCHEMA_VERSION,
    StoryArcBeat, StoryArcBeatKind, StoryCameraMode, StoryCategory, StoryConfidence,
    StoryConfidenceLevel, StoryDocument, StoryEvidence, StoryEvidenceKind, StoryFraming,
    StoryParticipant, StoryParticipantRole, StoryShot, StoryShotFallback, StorySwitchWindow,
    StoryTakeRole, TimelineDocument,
};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

const TREE_PATTERN_ID: &str = "hoodwink_ground_acorn_quelling_blade";
const BUYBACK_DIEBACK_KIND: &str = "buyback_dieback";
const MAX_REACTION_SHOTS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoryValidationError {
    pub path: String,
    pub message: String,
}

impl StoryValidationError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for StoryValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl Error for StoryValidationError {}

pub fn build_story_document(
    timeline: &TimelineDocument,
    highlights: &HighlightDocument,
) -> Result<StoryDocument, StoryValidationError> {
    if timeline.source_sha256 != highlights.source_sha256 {
        return Err(StoryValidationError::new(
            "source_sha256",
            "timeline and highlights do not describe the same replay",
        ));
    }

    let mut stories = Vec::new();
    stories.extend(build_tree_counter_stories(timeline, highlights));
    stories.extend(build_kill_sequence_stories(highlights));
    stories.extend(build_buyback_dieback_stories(timeline, highlights));
    stories.sort_by(|left, right| {
        right
            .priority_score
            .total_cmp(&left.priority_score)
            .then_with(|| {
                left.source_start_seconds
                    .total_cmp(&right.source_start_seconds)
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    for (index, story) in stories.iter_mut().enumerate() {
        story.rank = index + 1;
    }

    let document = StoryDocument {
        schema_version: STORY_SCHEMA_VERSION.to_string(),
        source_sha256: timeline.source_sha256.clone(),
        detector: highlights.detector.clone(),
        stories,
    };
    validate_story_document(&document, highlights)?;
    Ok(document)
}

fn build_tree_counter_stories(
    timeline: &TimelineDocument,
    highlights: &HighlightDocument,
) -> Vec<HighlightStory> {
    let mut groups: BTreeMap<(String, String), Vec<&HighlightCandidate>> = BTreeMap::new();
    for candidate in &highlights.candidates {
        let Some(interaction) = candidate.interaction.as_ref() else {
            continue;
        };
        let Some(primary_hero) = candidate.primary_hero.as_ref() else {
            continue;
        };
        if candidate.kind != "mechanical_counterplay"
            || interaction.pattern_id != TREE_PATTERN_ID
            || interaction.verification.is_none()
        {
            continue;
        }
        groups
            .entry((interaction.pattern_id.clone(), primary_hero.clone()))
            .or_default()
            .push(candidate);
    }

    groups
        .into_iter()
        .filter_map(|((pattern_id, primary_hero), mut candidates)| {
            candidates.sort_by_key(|candidate| {
                candidate
                    .interaction
                    .as_ref()
                    .map(|interaction| interaction.occurrence_index)
                    .unwrap_or(usize::MAX)
            });
            build_tree_counter_story(timeline, &pattern_id, &primary_hero, &candidates)
        })
        .collect()
}

fn build_tree_counter_story(
    timeline: &TimelineDocument,
    pattern_id: &str,
    primary_hero: &str,
    candidates: &[&HighlightCandidate],
) -> Option<HighlightStory> {
    let first = candidates.first()?;
    let story_id = format!("story-comedy-{}-{}", hero_slug(primary_hero), first.id);
    let source_hero = "npc_dota_hero_hoodwink";
    let representative_id = candidates
        .iter()
        .find(|candidate| {
            candidate
                .reasons
                .iter()
                .any(|reason| reason.contains("representative moment"))
        })
        .map(|candidate| candidate.id.as_str());
    let mut evidence = Vec::new();
    let mut beats = Vec::new();
    let mut shot_drafts = Vec::new();
    let mut switch_windows = Vec::new();

    for (index, candidate) in candidates.iter().enumerate() {
        let interaction = candidate.interaction.as_ref()?;
        let verification = interaction.verification.as_ref()?;
        let occurrence = interaction.occurrence_index;
        let trigger_time = candidate.peak_seconds - interaction.response_delay_seconds;
        let trigger_event = nearest_action_event(
            timeline,
            source_hero,
            &interaction.trigger_name,
            trigger_time,
            candidate.start_seconds,
            candidate.peak_seconds,
        );
        let trigger_id = format!("{story_id}-e-cut-{occurrence:02}-trigger");
        let response_id = format!("{story_id}-e-cut-{occurrence:02}-response");
        let verification_id = format!("{story_id}-e-cut-{occurrence:02}-proof");
        evidence.push(StoryEvidence {
            id: trigger_id.clone(),
            kind: StoryEvidenceKind::Trigger,
            candidate_id: Some(candidate.id.clone()),
            tick: trigger_event.map(|event| event.tick),
            time_seconds: trigger_event
                .and_then(|event| event.time_seconds)
                .unwrap_or(trigger_time),
            actor: Some(source_hero.to_string()),
            target: None,
            action: Some(interaction.trigger_name.clone()),
            detail: "Ground-targeted Acorn Shot created the temporary tree.".to_string(),
        });
        evidence.push(StoryEvidence {
            id: response_id.clone(),
            kind: StoryEvidenceKind::Response,
            candidate_id: Some(candidate.id.clone()),
            tick: Some(candidate.anchor_tick),
            time_seconds: candidate.peak_seconds,
            actor: Some(primary_hero.to_string()),
            target: Some(source_hero.to_string()),
            action: Some(interaction.response_name.clone()),
            detail: format!(
                "Quelling Blade response followed {:.2}s after the trigger.",
                interaction.response_delay_seconds
            ),
        });
        evidence.push(StoryEvidence {
            id: verification_id.clone(),
            kind: StoryEvidenceKind::Verification,
            candidate_id: Some(candidate.id.clone()),
            tick: Some(verification.tree_deleted_tick),
            time_seconds: candidate.peak_seconds,
            actor: Some(primary_hero.to_string()),
            target: None,
            action: Some(verification.method.clone()),
            detail: format!(
                "Tree handle {} was created at tick {}, targeted at tick {}, and deleted at tick {}.",
                verification.tree_entity_handle,
                verification.tree_created_tick,
                verification.matched_tree_order_tick,
                verification.tree_deleted_tick
            ),
        });

        let is_final = index + 1 == candidates.len();
        let is_representative = representative_id == Some(candidate.id.as_str());
        let beat_kind = if index == 0 {
            StoryArcBeatKind::Setup
        } else if is_final {
            StoryArcBeatKind::Payoff
        } else if is_representative {
            StoryArcBeatKind::Turn
        } else {
            StoryArcBeatKind::Development
        };
        let beat_id = format!("{story_id}-beat-cut-{occurrence:02}");
        beats.push(StoryArcBeat {
            id: beat_id.clone(),
            kind: beat_kind,
            source_start_seconds: candidate.start_seconds,
            source_end_seconds: candidate.end_seconds,
            summary: format!(
                "Verified temporary-tree counter {occurrence}/{}.",
                candidates.len()
            ),
            evidence_ids: vec![trigger_id, response_id, verification_id],
        });
        let take_group_id = format!("{story_id}-take-cut-{occurrence:02}");
        shot_drafts.push(ShotDraft {
            id: format!("{take_group_id}-primary"),
            source_order: candidate.start_seconds,
            sub_order: 0,
            take_group_id: take_group_id.clone(),
            take_role: StoryTakeRole::Primary,
            include_in_default_cut: true,
            beat_id: beat_id.clone(),
            candidate_id: candidate.id.clone(),
            source_start_seconds: candidate.start_seconds,
            source_end_seconds: candidate.end_seconds,
            camera_mode: StoryCameraMode::PlayerPerspective,
            target_hero: Some(primary_hero.to_string()),
            framing: StoryFraming::NormalGameplay,
            purpose: "Keep the complete setup and response in the player's normal view."
                .to_string(),
            fallback: player_fallback(primary_hero, "Player view is already the safe shot."),
        });
        if is_representative || is_final {
            let alternate_id = format!("{take_group_id}-close");
            shot_drafts.push(ShotDraft {
                id: alternate_id.clone(),
                source_order: candidate.start_seconds,
                sub_order: 1,
                take_group_id: take_group_id.clone(),
                take_role: StoryTakeRole::Alternate,
                include_in_default_cut: false,
                beat_id,
                candidate_id: candidate.id.clone(),
                source_start_seconds: candidate.start_seconds,
                source_end_seconds: candidate.end_seconds,
                camera_mode: StoryCameraMode::HeroChase,
                target_hero: Some(primary_hero.to_string()),
                framing: StoryFraming::CloseAction,
                purpose: "Capture a synchronized close-camera take for the verified tree cut."
                    .to_string(),
                fallback: player_fallback(
                    primary_hero,
                    "Use the normal player view if hero chase loses the action.",
                ),
            });
            switch_windows.push(StorySwitchWindow {
                take_group_id,
                alternate_shot_id: alternate_id,
                source_start_seconds: (candidate.peak_seconds - 1.2).max(candidate.start_seconds),
                source_end_seconds: (candidate.peak_seconds + 1.8).min(candidate.end_seconds),
                reason: "Suggested close-camera window around the verified tree deletion."
                    .to_string(),
            });
        }
    }

    let reaction_events = salutes_between_heroes(timeline, source_hero, primary_hero);
    for (index, salute) in reaction_events.iter().take(MAX_REACTION_SHOTS).enumerate() {
        let reaction_time = event_time_near_tick(timeline, salute.tick)?;
        let nearest_candidate = candidates.iter().min_by(|left, right| {
            (left.peak_seconds - reaction_time)
                .abs()
                .total_cmp(&(right.peak_seconds - reaction_time).abs())
        })?;
        let evidence_id = format!("{story_id}-e-reaction-{:02}", index + 1);
        let beat_id = format!("{story_id}-beat-reaction-{:02}", index + 1);
        evidence.push(StoryEvidence {
            id: evidence_id.clone(),
            kind: StoryEvidenceKind::Reaction,
            candidate_id: Some(nearest_candidate.id.clone()),
            tick: Some(salute.tick),
            time_seconds: reaction_time,
            actor: Some(source_hero.to_string()),
            target: Some(primary_hero.to_string()),
            action: Some("player_tip".to_string()),
            detail: "The tree creator tipped the responder after the repeated counterplay."
                .to_string(),
        });
        beats.push(StoryArcBeat {
            id: beat_id.clone(),
            kind: StoryArcBeatKind::Reaction,
            source_start_seconds: (reaction_time - 1.0).max(0.0),
            source_end_seconds: (reaction_time + 2.0).min(timeline.replay.playback_time_seconds),
            summary: "In-game tip acknowledges the repeated counterplay.".to_string(),
            evidence_ids: vec![evidence_id],
        });
        shot_drafts.push(ShotDraft {
            id: format!("{story_id}-take-reaction-{:02}-primary", index + 1),
            source_order: reaction_time - 1.0,
            sub_order: 2,
            take_group_id: format!("{story_id}-take-reaction-{:02}", index + 1),
            take_role: StoryTakeRole::Primary,
            include_in_default_cut: true,
            beat_id,
            candidate_id: nearest_candidate.id.clone(),
            source_start_seconds: (reaction_time - 1.0).max(0.0),
            source_end_seconds: (reaction_time + 2.0).min(timeline.replay.playback_time_seconds),
            camera_mode: StoryCameraMode::PlayerPerspective,
            target_hero: Some(primary_hero.to_string()),
            framing: StoryFraming::NormalGameplay,
            purpose: "Preserve the game-audio reaction without adding narration.".to_string(),
            fallback: player_fallback(primary_hero, "Keep the same player view."),
        });
    }

    beats.sort_by(|left, right| {
        left.source_start_seconds
            .total_cmp(&right.source_start_seconds)
            .then_with(|| left.id.cmp(&right.id))
    });
    let shots = finalize_shots(shot_drafts);
    let source_start_seconds = beats
        .first()
        .map(|beat| beat.source_start_seconds)
        .unwrap_or(first.start_seconds);
    let source_end_seconds = beats
        .iter()
        .map(|beat| beat.source_end_seconds)
        .max_by(f32::total_cmp)
        .unwrap_or(first.end_seconds);
    let representative = candidates
        .iter()
        .find(|candidate| representative_id == Some(candidate.id.as_str()))
        .copied()
        .unwrap_or(first);
    let salute_count = reaction_events.len();

    Some(HighlightStory {
        id: story_id,
        rank: 0,
        category: StoryCategory::Comedy,
        template_id: pattern_id.to_string(),
        title: format!(
            "{} repeated Quelling Blade tree counter ({} times)",
            hero_slug(primary_hero),
            candidates.len()
        ),
        primary_hero: primary_hero.to_string(),
        participants: vec![
            StoryParticipant {
                hero: primary_hero.to_string(),
                role: StoryParticipantRole::Protagonist,
            },
            StoryParticipant {
                hero: source_hero.to_string(),
                role: StoryParticipantRole::Opponent,
            },
        ],
        candidate_ids: candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect(),
        source_start_seconds,
        source_peak_seconds: representative.peak_seconds,
        source_end_seconds,
        priority_score: representative.score
            + candidates.len() as f32 * 12.0
            + salute_count.min(3) as f32 * 5.0,
        confidence: StoryConfidence {
            level: StoryConfidenceLevel::High,
            score: 0.99,
            reasons: vec![
                "Every occurrence has a matched tree-target order and temporary-tree deletion."
                    .to_string(),
                format!(
                    "The same responder repeated the verified action {} times.",
                    candidates.len()
                ),
            ],
        },
        review_required: false,
        evidence,
        beats,
        shots,
        switch_windows,
        reasons: vec![
            format!(
                "{} verified repetitions create a readable running gag.",
                candidates.len()
            ),
            format!("{salute_count} opponent tip reaction(s) are available as social context."),
        ],
    })
}

fn build_kill_sequence_stories(highlights: &HighlightDocument) -> Vec<HighlightStory> {
    highlights
        .candidates
        .iter()
        .filter_map(build_kill_sequence_story)
        .collect()
}

fn build_kill_sequence_story(candidate: &HighlightCandidate) -> Option<HighlightStory> {
    let sequence = candidate.kill_sequence.as_ref()?;
    if candidate.kind != "hero_kill_sequence" || sequence.kills.is_empty() {
        return None;
    }
    let story_id = format!("story-skill-{}", candidate.id);
    let primary_hero = sequence.hero.as_str();
    let mut participants = vec![StoryParticipant {
        hero: primary_hero.to_string(),
        role: StoryParticipantRole::Protagonist,
    }];
    for target in sequence
        .kills
        .iter()
        .map(|kill| kill.target_hero.as_str())
        .collect::<BTreeSet<_>>()
    {
        participants.push(StoryParticipant {
            hero: target.to_string(),
            role: StoryParticipantRole::Target,
        });
    }

    let mut evidence = Vec::new();
    for (index, kill) in sequence.kills.iter().enumerate() {
        let number = index + 1;
        evidence.push(StoryEvidence {
            id: format!("{story_id}-e-kill-{number:02}-setup"),
            kind: StoryEvidenceKind::Trigger,
            candidate_id: Some(candidate.id.clone()),
            tick: Some(kill.setup_tick),
            time_seconds: kill.setup_time_seconds,
            actor: Some(primary_hero.to_string()),
            target: Some(kill.target_hero.clone()),
            action: kill.setup_action.clone(),
            detail: "Replay-attributed action begins the kill chain.".to_string(),
        });
        evidence.push(StoryEvidence {
            id: format!("{story_id}-e-kill-{number:02}-death"),
            kind: StoryEvidenceKind::Kill,
            candidate_id: Some(candidate.id.clone()),
            tick: Some(kill.death_tick),
            time_seconds: kill.death_time_seconds,
            actor: Some(primary_hero.to_string()),
            target: Some(kill.target_hero.clone()),
            action: kill.inflictor.clone(),
            detail: "The selected hero received kill credit at this death tick.".to_string(),
        });
    }

    let mut grouped: Vec<Vec<(usize, &HeroKillMoment)>> = Vec::new();
    for (index, kill) in sequence.kills.iter().enumerate() {
        if grouped
            .last()
            .and_then(|group| group.first())
            .is_some_and(|(_, first)| first.setup_tick == kill.setup_tick)
        {
            grouped
                .last_mut()
                .expect("group exists")
                .push((index, kill));
        } else {
            grouped.push(vec![(index, kill)]);
        }
    }

    let first_setup = sequence.kills[0].setup_time_seconds;
    let mut beats = vec![StoryArcBeat {
        id: format!("{story_id}-beat-setup"),
        kind: StoryArcBeatKind::Setup,
        source_start_seconds: candidate.start_seconds,
        source_end_seconds: first_setup.max(candidate.start_seconds + 0.1),
        summary: "Player-view context before the attributed action.".to_string(),
        evidence_ids: vec![format!("{story_id}-e-kill-01-setup")],
    }];
    let mut kill_beat_ids = BTreeMap::new();
    for (group_index, group) in grouped.iter().enumerate() {
        let beat_id = format!("{story_id}-beat-chain-{:02}", group_index + 1);
        let is_final = group_index + 1 == grouped.len();
        let end = group
            .iter()
            .map(|(_, kill)| kill.death_time_seconds)
            .max_by(f32::total_cmp)
            .unwrap_or(candidate.peak_seconds);
        let mut evidence_ids = Vec::new();
        for (kill_index, _) in group {
            evidence_ids.push(format!("{story_id}-e-kill-{:02}-setup", kill_index + 1));
            evidence_ids.push(format!("{story_id}-e-kill-{:02}-death", kill_index + 1));
            kill_beat_ids.insert(*kill_index, beat_id.clone());
        }
        beats.push(StoryArcBeat {
            id: beat_id,
            kind: if is_final {
                StoryArcBeatKind::Payoff
            } else {
                StoryArcBeatKind::Development
            },
            source_start_seconds: group[0].1.setup_time_seconds,
            source_end_seconds: (end + 1.25).min(candidate.end_seconds),
            summary: format!(
                "Attributed action resolves into {} confirmed kill(s).",
                group.len()
            ),
            evidence_ids,
        });
    }

    let take_group_id = format!("{story_id}-take-sequence");
    let mut shot_drafts = vec![ShotDraft {
        id: format!("{take_group_id}-primary"),
        source_order: candidate.start_seconds,
        sub_order: 0,
        take_group_id: take_group_id.clone(),
        take_role: StoryTakeRole::Primary,
        include_in_default_cut: true,
        beat_id: format!("{story_id}-beat-setup"),
        candidate_id: candidate.id.clone(),
        source_start_seconds: candidate.start_seconds,
        source_end_seconds: candidate.end_seconds,
        camera_mode: StoryCameraMode::PlayerPerspective,
        target_hero: Some(primary_hero.to_string()),
        framing: StoryFraming::NormalGameplay,
        purpose: "Keep the full kill sequence continuous in the selected player's view."
            .to_string(),
        fallback: player_fallback(primary_hero, "Player view is already the safe shot."),
    }];
    let close_index = sequence
        .kills
        .iter()
        .enumerate()
        .rev()
        .find(|(_, kill)| {
            kill.setup_action
                .as_deref()
                .or(kill.inflictor.as_deref())
                .is_some_and(camera_worthy_action)
        })
        .map(|(index, _)| index)
        .or_else(|| (sequence.kills.len() > 1).then_some(sequence.kills.len() - 1));
    let mut switch_windows = Vec::new();
    if let Some(close_index) = close_index {
        let close_kill = &sequence.kills[close_index];
        let alternate_id = format!("{take_group_id}-close");
        shot_drafts.push(ShotDraft {
            id: alternate_id.clone(),
            source_order: candidate.start_seconds,
            sub_order: 1,
            take_group_id: take_group_id.clone(),
            take_role: StoryTakeRole::Alternate,
            include_in_default_cut: false,
            beat_id: kill_beat_ids
                .get(&close_index)
                .cloned()
                .unwrap_or_else(|| format!("{story_id}-beat-setup")),
            candidate_id: candidate.id.clone(),
            source_start_seconds: candidate.start_seconds,
            source_end_seconds: candidate.end_seconds,
            camera_mode: StoryCameraMode::HeroChase,
            target_hero: Some(close_kill.target_hero.clone()),
            framing: StoryFraming::CloseAction,
            purpose: "Capture the same sequence as a synchronized close-camera source take."
                .to_string(),
            fallback: player_fallback(
                primary_hero,
                "Return to the protagonist's player view if the target cannot be framed.",
            ),
        });
        switch_windows.push(StorySwitchWindow {
            take_group_id: take_group_id.clone(),
            alternate_shot_id: alternate_id,
            source_start_seconds: (close_kill.death_time_seconds - 1.25)
                .max(candidate.start_seconds),
            source_end_seconds: (close_kill.death_time_seconds + 1.5).min(candidate.end_seconds),
            reason: "Suggested close-camera switch around the decisive impact.".to_string(),
        });
    }

    let arrow_kills = sequence
        .kills
        .iter()
        .filter(|kill| {
            kill.setup_action
                .as_deref()
                .or(kill.inflictor.as_deref())
                .is_some_and(|action| action.contains("arrow"))
        })
        .count();
    let complete_setup_count = sequence
        .kills
        .iter()
        .filter(|kill| kill.setup_action.is_some())
        .count();
    Some(HighlightStory {
        id: story_id.clone(),
        rank: 0,
        category: StoryCategory::Skill,
        template_id: "hero_kill_sequence_v1".to_string(),
        title: format!(
            "{} confirmed kill sequence ({} kills)",
            hero_slug(primary_hero),
            sequence.kills.len()
        ),
        primary_hero: primary_hero.to_string(),
        participants,
        candidate_ids: vec![candidate.id.clone()],
        source_start_seconds: candidate.start_seconds,
        source_peak_seconds: candidate.peak_seconds,
        source_end_seconds: candidate.end_seconds,
        priority_score: candidate.score
            + sequence.kills.len() as f32 * 10.0
            + arrow_kills as f32 * 8.0,
        confidence: StoryConfidence {
            level: StoryConfidenceLevel::High,
            score: if complete_setup_count == sequence.kills.len() {
                0.97
            } else {
                0.92
            },
            reasons: vec![
                "Every payoff is a replay-attributed hero death.".to_string(),
                format!(
                    "{} of {} kills have an explicit setup action.",
                    complete_setup_count,
                    sequence.kills.len()
                ),
            ],
        },
        review_required: false,
        evidence,
        beats,
        shots: finalize_shots(shot_drafts),
        switch_windows,
        reasons: vec![
            format!(
                "{} kill(s) remain in one continuous engagement.",
                sequence.kills.len()
            ),
            format!("{arrow_kills} arrow-related payoff(s) support a technical edit."),
        ],
    })
}

fn build_buyback_dieback_stories(
    timeline: &TimelineDocument,
    highlights: &HighlightDocument,
) -> Vec<HighlightStory> {
    highlights
        .candidates
        .iter()
        .filter(|candidate| candidate.kind == BUYBACK_DIEBACK_KIND)
        .filter_map(|candidate| build_buyback_dieback_story(timeline, candidate))
        .collect()
}

fn build_buyback_dieback_story(
    timeline: &TimelineDocument,
    candidate: &HighlightCandidate,
) -> Option<HighlightStory> {
    let primary_hero = candidate.primary_hero.as_deref()?;
    let slot = timeline
        .replay
        .players
        .iter()
        .find(|player| player.hero_name == primary_hero)
        .map(|player| u32::from(player.slot))?;
    let death = timeline.events.iter().find(|event| {
        event.event_type == "DotaCombatlogDeath"
            && event.tick == candidate.anchor_tick
            && event.target.as_deref() == Some(primary_hero)
            && event.will_reincarnate != Some(true)
    })?;
    let death_time = death.time_seconds?;
    let buyback = timeline
        .events
        .iter()
        .filter(|event| {
            event.event_type == "DotaCombatlogBuyback"
                && event.value == Some(slot)
                && event
                    .time_seconds
                    .is_some_and(|time| time < death_time && death_time - time <= 90.0)
        })
        .max_by(|left, right| {
            left.time_seconds
                .unwrap_or_default()
                .total_cmp(&right.time_seconds.unwrap_or_default())
        })?;
    let buyback_time = buyback.time_seconds?;
    let delay = death_time - buyback_time;
    let story_id = format!("story-mistake-{}", candidate.id);
    let killer = death
        .attacker
        .as_deref()
        .filter(|hero| hero.starts_with("npc_dota_hero_"));
    let mut participants = vec![StoryParticipant {
        hero: primary_hero.to_string(),
        role: StoryParticipantRole::Protagonist,
    }];
    if let Some(killer) = killer {
        participants.push(StoryParticipant {
            hero: killer.to_string(),
            role: StoryParticipantRole::Opponent,
        });
    }
    let setup_id = format!("{story_id}-e-buyback");
    let payoff_id = format!("{story_id}-e-death");
    let setup_beat_id = format!("{story_id}-beat-setup");
    let turn_beat_id = format!("{story_id}-beat-turn");
    let payoff_beat_id = format!("{story_id}-beat-payoff");
    let setup_end = (buyback_time + 1.5).min(death_time - 0.1);
    let turn_start = setup_end.max(death_time - 5.0);
    let mut beats = vec![StoryArcBeat {
        id: setup_beat_id.clone(),
        kind: StoryArcBeatKind::Setup,
        source_start_seconds: candidate.start_seconds,
        source_end_seconds: setup_end,
        summary: "The player commits buyback.".to_string(),
        evidence_ids: vec![setup_id.clone()],
    }];
    if death_time - turn_start >= 0.1 {
        beats.push(StoryArcBeat {
            id: turn_beat_id.clone(),
            kind: StoryArcBeatKind::Turn,
            source_start_seconds: turn_start,
            source_end_seconds: death_time,
            summary: "The returned hero is placed under lethal pressure.".to_string(),
            evidence_ids: vec![setup_id.clone(), payoff_id.clone()],
        });
    }
    beats.push(StoryArcBeat {
        id: payoff_beat_id.clone(),
        kind: StoryArcBeatKind::Payoff,
        source_start_seconds: death_time,
        source_end_seconds: candidate.end_seconds,
        summary: format!("The hero dies again {delay:.1}s after buyback."),
        evidence_ids: vec![payoff_id.clone()],
    });

    let mut shot_drafts = Vec::new();
    let (alternate_group_id, alternate_start, alternate_end) = if delay <= 15.0 {
        let group_id = format!("{story_id}-take-immediate");
        shot_drafts.push(ShotDraft {
            id: format!("{group_id}-primary"),
            source_order: candidate.start_seconds,
            sub_order: 0,
            take_group_id: group_id.clone(),
            take_role: StoryTakeRole::Primary,
            include_in_default_cut: true,
            beat_id: setup_beat_id,
            candidate_id: candidate.id.clone(),
            source_start_seconds: candidate.start_seconds,
            source_end_seconds: candidate.end_seconds,
            camera_mode: StoryCameraMode::PlayerPerspective,
            target_hero: Some(primary_hero.to_string()),
            framing: StoryFraming::NormalGameplay,
            purpose: "Keep the immediate buyback-to-death consequence continuous.".to_string(),
            fallback: player_fallback(primary_hero, "Player view is already the safe shot."),
        });
        (group_id, candidate.start_seconds, candidate.end_seconds)
    } else {
        let setup_group_id = format!("{story_id}-take-buyback");
        shot_drafts.push(ShotDraft {
            id: format!("{setup_group_id}-primary"),
            source_order: candidate.start_seconds,
            sub_order: 0,
            take_group_id: setup_group_id,
            take_role: StoryTakeRole::Primary,
            include_in_default_cut: true,
            beat_id: setup_beat_id,
            candidate_id: candidate.id.clone(),
            source_start_seconds: candidate.start_seconds,
            source_end_seconds: (buyback_time + 3.0).min(candidate.end_seconds),
            camera_mode: StoryCameraMode::PlayerPerspective,
            target_hero: Some(primary_hero.to_string()),
            framing: StoryFraming::NormalGameplay,
            purpose: "Establish the buyback without retaining the inactive gap.".to_string(),
            fallback: player_fallback(primary_hero, "Player view is already the safe shot."),
        });
        let payoff_group_id = format!("{story_id}-take-payoff");
        let payoff_start = (death_time - 6.0).max(candidate.start_seconds);
        shot_drafts.push(ShotDraft {
            id: format!("{payoff_group_id}-primary"),
            source_order: payoff_start,
            sub_order: 0,
            take_group_id: payoff_group_id.clone(),
            take_role: StoryTakeRole::Primary,
            include_in_default_cut: true,
            beat_id: turn_beat_id,
            candidate_id: candidate.id.clone(),
            source_start_seconds: payoff_start,
            source_end_seconds: candidate.end_seconds,
            camera_mode: StoryCameraMode::PlayerPerspective,
            target_hero: Some(primary_hero.to_string()),
            framing: StoryFraming::NormalGameplay,
            purpose: "Return directly to the punishment and omit the inactive middle.".to_string(),
            fallback: player_fallback(primary_hero, "Player view is already the safe shot."),
        });
        (payoff_group_id, payoff_start, candidate.end_seconds)
    };
    let alternate_id = format!("{alternate_group_id}-close");
    shot_drafts.push(ShotDraft {
        id: alternate_id.clone(),
        source_order: alternate_start,
        sub_order: 1,
        take_group_id: alternate_group_id.clone(),
        take_role: StoryTakeRole::Alternate,
        include_in_default_cut: false,
        beat_id: payoff_beat_id,
        candidate_id: candidate.id.clone(),
        source_start_seconds: alternate_start,
        source_end_seconds: alternate_end,
        camera_mode: StoryCameraMode::HeroChase,
        target_hero: Some(primary_hero.to_string()),
        framing: StoryFraming::CloseAction,
        purpose: "Capture the same consequence interval as a synchronized close take.".to_string(),
        fallback: player_fallback(
            primary_hero,
            "Use player view if the close camera cannot keep the hero visible.",
        ),
    });

    let confidence_level = if delay <= 20.0 {
        StoryConfidenceLevel::High
    } else {
        StoryConfidenceLevel::Medium
    };
    Some(HighlightStory {
        id: story_id.clone(),
        rank: 0,
        category: StoryCategory::Mistake,
        template_id: "buyback_dieback_v1".to_string(),
        title: format!("{} died {delay:.1}s after buyback", hero_slug(primary_hero)),
        primary_hero: primary_hero.to_string(),
        participants,
        candidate_ids: vec![candidate.id.clone()],
        source_start_seconds: candidate.start_seconds,
        source_peak_seconds: death_time,
        source_end_seconds: candidate.end_seconds,
        priority_score: candidate.score + (90.0 - delay).max(0.0),
        confidence: StoryConfidence {
            level: confidence_level,
            score: if delay <= 20.0 { 0.98 } else { 0.78 },
            reasons: vec![
                "The buyback event is mapped to the replay player slot.".to_string(),
                "The same hero has a non-reincarnation death within 90 seconds.".to_string(),
                format!("Measured buyback-to-death delay: {delay:.1}s."),
            ],
        },
        review_required: confidence_level != StoryConfidenceLevel::High,
        evidence: vec![
            StoryEvidence {
                id: setup_id,
                kind: StoryEvidenceKind::Trigger,
                candidate_id: Some(candidate.id.clone()),
                tick: Some(buyback.tick),
                time_seconds: buyback_time,
                actor: Some(primary_hero.to_string()),
                target: None,
                action: Some("buyback".to_string()),
                detail: "Replay buyback event is attributed by player slot.".to_string(),
            },
            StoryEvidence {
                id: payoff_id,
                kind: StoryEvidenceKind::Kill,
                candidate_id: Some(candidate.id.clone()),
                tick: Some(death.tick),
                time_seconds: death_time,
                actor: killer.map(ToOwned::to_owned),
                target: Some(primary_hero.to_string()),
                action: death.inflictor.clone(),
                detail: "The same hero dies again without reincarnation.".to_string(),
            },
        ],
        beats,
        shots: finalize_shots(shot_drafts),
        switch_windows: vec![StorySwitchWindow {
            take_group_id: alternate_group_id,
            alternate_shot_id: alternate_id,
            source_start_seconds: (death_time - 1.25).max(alternate_start),
            source_end_seconds: (death_time + 1.5).min(alternate_end),
            reason: "Suggested close-camera switch around the dieback.".to_string(),
        }],
        reasons: vec![
            "Buyback followed by another death is an objective consequence, not guessed intent."
                .to_string(),
            if confidence_level == StoryConfidenceLevel::High {
                "The short delay is strong enough for automatic promotion.".to_string()
            } else {
                "The longer delay requires human review before calling it a mistake.".to_string()
            },
        ],
    })
}

#[derive(Clone)]
struct ShotDraft {
    id: String,
    source_order: f32,
    sub_order: usize,
    take_group_id: String,
    take_role: StoryTakeRole,
    include_in_default_cut: bool,
    beat_id: String,
    candidate_id: String,
    source_start_seconds: f32,
    source_end_seconds: f32,
    camera_mode: StoryCameraMode,
    target_hero: Option<String>,
    framing: StoryFraming,
    purpose: String,
    fallback: StoryShotFallback,
}

fn finalize_shots(mut drafts: Vec<ShotDraft>) -> Vec<StoryShot> {
    drafts.sort_by(|left, right| {
        left.source_order
            .total_cmp(&right.source_order)
            .then_with(|| left.sub_order.cmp(&right.sub_order))
            .then_with(|| left.candidate_id.cmp(&right.candidate_id))
    });
    drafts
        .into_iter()
        .enumerate()
        .map(|(index, draft)| StoryShot {
            id: draft.id,
            order: index + 1,
            take_group_id: draft.take_group_id,
            take_role: draft.take_role,
            include_in_default_cut: draft.include_in_default_cut,
            beat_id: draft.beat_id,
            candidate_id: draft.candidate_id,
            source_start_seconds: draft.source_start_seconds,
            source_end_seconds: draft.source_end_seconds,
            camera_mode: draft.camera_mode,
            target_hero: draft.target_hero,
            framing: draft.framing,
            purpose: draft.purpose,
            fallback: draft.fallback,
        })
        .collect()
}

fn player_fallback(primary_hero: &str, reason: &str) -> StoryShotFallback {
    StoryShotFallback {
        camera_mode: StoryCameraMode::PlayerPerspective,
        target_hero: Some(primary_hero.to_string()),
        reason: reason.to_string(),
    }
}

fn hero_slug(hero: &str) -> String {
    hero.strip_prefix("npc_dota_hero_")
        .unwrap_or(hero)
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn camera_worthy_action(action: &str) -> bool {
    [
        "arrow",
        "hook",
        "shackleshot",
        "powershot",
        "sun_strike",
        "spear",
        "sharpshooter",
        "assassinate",
        "laguna_blade",
        "finger_of_death",
    ]
    .iter()
    .any(|cue| action.contains(cue))
}

fn nearest_action_event<'a>(
    timeline: &'a TimelineDocument,
    actor: &str,
    action: &str,
    expected_time: f32,
    start: f32,
    end: f32,
) -> Option<&'a d2_highlights_core::CombatEvent> {
    timeline
        .events
        .iter()
        .filter(|event| {
            event.attacker.as_deref() == Some(actor)
                && event.inflictor.as_deref() == Some(action)
                && event
                    .time_seconds
                    .is_some_and(|time| time >= start && time <= end)
        })
        .min_by(|left, right| {
            (left.time_seconds.unwrap_or_default() - expected_time)
                .abs()
                .total_cmp(&(right.time_seconds.unwrap_or_default() - expected_time).abs())
        })
}

fn salutes_between_heroes<'a>(
    timeline: &'a TimelineDocument,
    source_hero: &str,
    target_hero: &str,
) -> Vec<&'a d2_highlights_core::PlayerSaluteEvent> {
    let slot = |hero: &str| {
        timeline
            .replay
            .players
            .iter()
            .find(|player| player.hero_name == hero)
            .map(|player| i32::from(player.slot))
    };
    let (Some(source_slot), Some(target_slot)) = (slot(source_hero), slot(target_hero)) else {
        return Vec::new();
    };
    let mut salutes = timeline
        .salutes
        .iter()
        .filter(|salute| {
            salute.source_player_id == Some(source_slot)
                && salute.target_player_id == Some(target_slot)
        })
        .collect::<Vec<_>>();
    salutes.sort_by_key(|salute| salute.tick);
    salutes
}

fn event_time_near_tick(timeline: &TimelineDocument, tick: u32) -> Option<f32> {
    timeline
        .events
        .iter()
        .filter_map(|event| {
            event
                .time_seconds
                .map(|time| (event.tick.abs_diff(tick), time))
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, time)| time)
}

pub fn validate_story_document(
    document: &StoryDocument,
    highlights: &HighlightDocument,
) -> Result<(), StoryValidationError> {
    if document.schema_version != STORY_SCHEMA_VERSION {
        return Err(StoryValidationError::new(
            "schema_version",
            format!("expected {STORY_SCHEMA_VERSION}"),
        ));
    }
    if document.source_sha256.is_empty() || document.source_sha256 != highlights.source_sha256 {
        return Err(StoryValidationError::new(
            "source_sha256",
            "story and highlight source hashes must match",
        ));
    }
    let candidate_ids = highlights
        .candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut story_ids = BTreeSet::new();
    for (story_index, story) in document.stories.iter().enumerate() {
        let path = format!("stories[{story_index}]");
        if story.rank != story_index + 1 {
            return Err(StoryValidationError::new(
                format!("{path}.rank"),
                "ranks must be contiguous and match document order",
            ));
        }
        if story.id.is_empty() || !story_ids.insert(story.id.as_str()) {
            return Err(StoryValidationError::new(
                format!("{path}.id"),
                "story IDs must be non-empty and unique",
            ));
        }
        validate_time_range(
            &format!("{path}.source"),
            story.source_start_seconds,
            story.source_end_seconds,
        )?;
        if !story.source_peak_seconds.is_finite()
            || story.source_peak_seconds < story.source_start_seconds
            || story.source_peak_seconds > story.source_end_seconds
        {
            return Err(StoryValidationError::new(
                format!("{path}.source_peak_seconds"),
                "peak must fall inside the story source range",
            ));
        }
        if !story.priority_score.is_finite() || story.priority_score < 0.0 {
            return Err(StoryValidationError::new(
                format!("{path}.priority_score"),
                "priority must be finite and non-negative",
            ));
        }
        if !story.confidence.score.is_finite() || !(0.0..=1.0).contains(&story.confidence.score) {
            return Err(StoryValidationError::new(
                format!("{path}.confidence.score"),
                "confidence score must be between zero and one",
            ));
        }
        if story.confidence.level == StoryConfidenceLevel::Low && !story.review_required {
            return Err(StoryValidationError::new(
                format!("{path}.review_required"),
                "low-confidence stories must require review",
            ));
        }
        if story.candidate_ids.is_empty()
            || story
                .candidate_ids
                .iter()
                .any(|candidate_id| !candidate_ids.contains(candidate_id.as_str()))
        {
            return Err(StoryValidationError::new(
                format!("{path}.candidate_ids"),
                "every story must reference existing highlight candidates",
            ));
        }
        let participant_heroes = story
            .participants
            .iter()
            .map(|participant| participant.hero.as_str())
            .collect::<BTreeSet<_>>();
        if story.primary_hero.is_empty()
            || !story.participants.iter().any(|participant| {
                participant.hero == story.primary_hero
                    && participant.role == StoryParticipantRole::Protagonist
            })
        {
            return Err(StoryValidationError::new(
                format!("{path}.primary_hero"),
                "primary hero must be a protagonist participant",
            ));
        }

        let mut evidence_ids = BTreeSet::new();
        for (evidence_index, item) in story.evidence.iter().enumerate() {
            let evidence_path = format!("{path}.evidence[{evidence_index}]");
            if item.id.is_empty() || !evidence_ids.insert(item.id.as_str()) {
                return Err(StoryValidationError::new(
                    format!("{evidence_path}.id"),
                    "evidence IDs must be non-empty and unique",
                ));
            }
            if !item.time_seconds.is_finite()
                || item.time_seconds < story.source_start_seconds - 0.01
                || item.time_seconds > story.source_end_seconds + 0.01
            {
                return Err(StoryValidationError::new(
                    format!("{evidence_path}.time_seconds"),
                    "evidence must fall inside the story source range",
                ));
            }
            if item.candidate_id.as_ref().is_some_and(|candidate_id| {
                !story.candidate_ids.iter().any(|id| id == candidate_id)
            }) {
                return Err(StoryValidationError::new(
                    format!("{evidence_path}.candidate_id"),
                    "evidence candidate must belong to the story",
                ));
            }
        }
        if evidence_ids.is_empty() {
            return Err(StoryValidationError::new(
                format!("{path}.evidence"),
                "stories require replay-derived evidence",
            ));
        }

        let mut beat_ids = BTreeSet::new();
        let mut previous_beat_start = f32::NEG_INFINITY;
        for (beat_index, beat) in story.beats.iter().enumerate() {
            let beat_path = format!("{path}.beats[{beat_index}]");
            if beat.id.is_empty() || !beat_ids.insert(beat.id.as_str()) {
                return Err(StoryValidationError::new(
                    format!("{beat_path}.id"),
                    "beat IDs must be non-empty and unique",
                ));
            }
            validate_time_range(
                &format!("{beat_path}.source"),
                beat.source_start_seconds,
                beat.source_end_seconds,
            )?;
            if beat.source_start_seconds < story.source_start_seconds - 0.01
                || beat.source_end_seconds > story.source_end_seconds + 0.01
            {
                return Err(StoryValidationError::new(
                    format!("{beat_path}.source"),
                    "beat must fall inside the story source range",
                ));
            }
            if beat.source_start_seconds + 0.001 < previous_beat_start {
                return Err(StoryValidationError::new(
                    format!("{beat_path}.source_start_seconds"),
                    "beats must be ordered by replay time",
                ));
            }
            previous_beat_start = beat.source_start_seconds;
            if beat.evidence_ids.is_empty()
                || beat
                    .evidence_ids
                    .iter()
                    .any(|evidence_id| !evidence_ids.contains(evidence_id.as_str()))
            {
                return Err(StoryValidationError::new(
                    format!("{beat_path}.evidence_ids"),
                    "beats must reference existing evidence",
                ));
            }
        }
        if beat_ids.is_empty() {
            return Err(StoryValidationError::new(
                format!("{path}.beats"),
                "stories require at least one beat",
            ));
        }

        let mut shot_ids = BTreeSet::new();
        let mut take_groups: BTreeMap<&str, Vec<&StoryShot>> = BTreeMap::new();
        let mut has_default_take = false;
        for (shot_index, shot) in story.shots.iter().enumerate() {
            let shot_path = format!("{path}.shots[{shot_index}]");
            if shot.order != shot_index + 1 {
                return Err(StoryValidationError::new(
                    format!("{shot_path}.order"),
                    "shot orders must be contiguous",
                ));
            }
            if shot.id.is_empty() || !shot_ids.insert(shot.id.as_str()) {
                return Err(StoryValidationError::new(
                    format!("{shot_path}.id"),
                    "shot IDs must be non-empty and unique",
                ));
            }
            if shot.take_group_id.is_empty() {
                return Err(StoryValidationError::new(
                    format!("{shot_path}.take_group_id"),
                    "take group ID must be non-empty",
                ));
            }
            take_groups
                .entry(shot.take_group_id.as_str())
                .or_default()
                .push(shot);
            match shot.take_role {
                StoryTakeRole::Primary if !shot.include_in_default_cut => {
                    return Err(StoryValidationError::new(
                        format!("{shot_path}.include_in_default_cut"),
                        "primary takes must be included in the default cut",
                    ));
                }
                StoryTakeRole::Alternate if shot.include_in_default_cut => {
                    return Err(StoryValidationError::new(
                        format!("{shot_path}.include_in_default_cut"),
                        "alternate takes must not be appended to the default cut",
                    ));
                }
                _ => {}
            }
            if !beat_ids.contains(shot.beat_id.as_str()) {
                return Err(StoryValidationError::new(
                    format!("{shot_path}.beat_id"),
                    "shot must reference an existing beat",
                ));
            }
            if !story
                .candidate_ids
                .iter()
                .any(|candidate_id| candidate_id == &shot.candidate_id)
            {
                return Err(StoryValidationError::new(
                    format!("{shot_path}.candidate_id"),
                    "shot candidate must belong to the story",
                ));
            }
            validate_time_range(
                &format!("{shot_path}.source"),
                shot.source_start_seconds,
                shot.source_end_seconds,
            )?;
            if shot.source_start_seconds < story.source_start_seconds - 0.01
                || shot.source_end_seconds > story.source_end_seconds + 0.01
            {
                return Err(StoryValidationError::new(
                    format!("{shot_path}.source"),
                    "shot must fall inside the story source range",
                ));
            }
            validate_camera_target(
                &format!("{shot_path}.camera"),
                shot.camera_mode,
                shot.target_hero.as_deref(),
                &participant_heroes,
            )?;
            validate_camera_target(
                &format!("{shot_path}.fallback"),
                shot.fallback.camera_mode,
                shot.fallback.target_hero.as_deref(),
                &participant_heroes,
            )?;
            has_default_take |=
                shot.take_role == StoryTakeRole::Primary && shot.include_in_default_cut;
        }
        if shot_ids.is_empty() || !has_default_take {
            return Err(StoryValidationError::new(
                format!("{path}.shots"),
                "stories require at least one default primary take",
            ));
        }
        for (take_group_id, takes) in &take_groups {
            let primary = takes
                .iter()
                .filter(|take| take.take_role == StoryTakeRole::Primary)
                .copied()
                .collect::<Vec<_>>();
            if primary.len() != 1 {
                return Err(StoryValidationError::new(
                    format!("{path}.take_groups.{take_group_id}"),
                    "every take group requires exactly one primary take",
                ));
            }
            let primary = primary[0];
            if takes.iter().any(|take| {
                take.candidate_id != primary.candidate_id
                    || (take.source_start_seconds - primary.source_start_seconds).abs() > 0.001
                    || (take.source_end_seconds - primary.source_end_seconds).abs() > 0.001
            }) {
                return Err(StoryValidationError::new(
                    format!("{path}.take_groups.{take_group_id}"),
                    "all camera takes in a group must use the same candidate and source interval",
                ));
            }
        }

        let mut alternates_with_window = BTreeSet::new();
        for (window_index, window) in story.switch_windows.iter().enumerate() {
            let window_path = format!("{path}.switch_windows[{window_index}]");
            validate_time_range(
                &format!("{window_path}.source"),
                window.source_start_seconds,
                window.source_end_seconds,
            )?;
            let alternate = story
                .shots
                .iter()
                .find(|shot| shot.id == window.alternate_shot_id)
                .ok_or_else(|| {
                    StoryValidationError::new(
                        format!("{window_path}.alternate_shot_id"),
                        "switch window must reference an existing alternate take",
                    )
                })?;
            if alternate.take_role != StoryTakeRole::Alternate
                || alternate.take_group_id != window.take_group_id
            {
                return Err(StoryValidationError::new(
                    window_path,
                    "switch window must reference an alternate in the same take group",
                ));
            }
            if window.source_start_seconds < alternate.source_start_seconds - 0.001
                || window.source_end_seconds > alternate.source_end_seconds + 0.001
            {
                return Err(StoryValidationError::new(
                    format!("{path}.switch_windows[{window_index}].source"),
                    "switch window must fall inside its synchronized take interval",
                ));
            }
            alternates_with_window.insert(alternate.id.as_str());
        }
        if story
            .shots
            .iter()
            .filter(|shot| shot.take_role == StoryTakeRole::Alternate)
            .any(|shot| !alternates_with_window.contains(shot.id.as_str()))
        {
            return Err(StoryValidationError::new(
                format!("{path}.switch_windows"),
                "every alternate take requires at least one suggested switch window",
            ));
        }
    }
    Ok(())
}

fn validate_time_range(path: &str, start: f32, end: f32) -> Result<(), StoryValidationError> {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
        return Err(StoryValidationError::new(
            path,
            "time range must be finite, non-negative, and increasing",
        ));
    }
    Ok(())
}

fn validate_camera_target(
    path: &str,
    mode: StoryCameraMode,
    target: Option<&str>,
    participants: &BTreeSet<&str>,
) -> Result<(), StoryValidationError> {
    if mode != StoryCameraMode::Directed && target.is_none() {
        return Err(StoryValidationError::new(
            path,
            "player and chase cameras require a target hero",
        ));
    }
    if target.is_some_and(|hero| !participants.contains(hero)) {
        return Err(StoryValidationError::new(
            path,
            "camera target must be a story participant",
        ));
    }
    Ok(())
}
