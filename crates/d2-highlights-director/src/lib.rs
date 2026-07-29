use d2_highlights_core::{
    AudioCue, AudioPlan, CameraPlan, DIRECTOR_SCHEMA_VERSION, DirectorDocument, DirectorSegment,
    HighlightCandidate, HighlightDocument, StoryBeat,
};

const TRANSITION_SECONDS: f32 = 0.35;
const MAX_SOURCE_PRE_PEAK_SECONDS: f32 = 20.0;
const MAX_SOURCE_POST_PEAK_SECONDS: f32 = 8.0;
const MIN_SEGMENT_SECONDS: f32 = 8.0;

pub fn build_director_plan(
    highlights: &HighlightDocument,
    template: &str,
    max_clips: usize,
    max_duration_seconds: f32,
) -> DirectorDocument {
    let mut selected: Vec<&HighlightCandidate> = Vec::new();
    for candidate in reserved_mechanical_story(highlights, max_clips) {
        if !selected
            .iter()
            .any(|existing| overlap_ratio(existing, candidate) >= 0.5)
        {
            selected.push(candidate);
        }
    }
    for candidate in &highlights.candidates {
        if selected.len() >= max_clips {
            break;
        }
        if candidate.kind == "hero_kill_sequence" {
            continue;
        }
        if selected.iter().any(|existing| existing.id == candidate.id) {
            continue;
        }
        if selected
            .iter()
            .any(|existing| overlap_ratio(existing, candidate) >= 0.5)
        {
            continue;
        }
        selected.push(candidate);
    }
    while selected.len() > 1 && estimated_total_duration(&selected) > max_duration_seconds {
        selected.pop();
    }
    selected.sort_by(|left, right| left.start_seconds.total_cmp(&right.start_seconds));

    let mut output_cursor = 0.0_f32;
    let mut segments = Vec::new();
    let mut cues = Vec::new();

    for candidate in selected {
        let source_start = candidate
            .start_seconds
            .max(candidate.peak_seconds - MAX_SOURCE_PRE_PEAK_SECONDS);
        let source_end = candidate
            .end_seconds
            .min(candidate.peak_seconds + MAX_SOURCE_POST_PEAK_SECONDS);
        let source_duration = source_end - source_start;
        let remaining = max_duration_seconds - output_cursor;
        let duration = source_duration.min(remaining);
        if duration < MIN_SEGMENT_SECONDS {
            continue;
        }

        let final_source_end = source_start + duration;
        let output_start = output_cursor;
        let output_end = output_start + duration;
        let local_peak = (candidate.peak_seconds - source_start).clamp(0.0, duration);
        let output_peak = output_start + local_peak;
        let primary = candidate.primary_hero.clone();

        let impact_start = (candidate.peak_seconds - 2.0).max(source_start);
        let impact_end = (candidate.peak_seconds + 2.5).min(final_source_end);
        let mut beats = Vec::new();
        if impact_start > source_start {
            beats.push(StoryBeat {
                kind: "setup".to_string(),
                source_start_seconds: source_start,
                source_end_seconds: impact_start,
                playback_speed: 1.0,
                camera: CameraPlan {
                    mode: "auto_directed".to_string(),
                    target_hero: primary.clone(),
                    framing: "combat_wide".to_string(),
                },
            });
        }
        beats.push(StoryBeat {
            kind: "impact".to_string(),
            source_start_seconds: impact_start,
            source_end_seconds: impact_end,
            playback_speed: if candidate.score >= 150.0 { 0.85 } else { 1.0 },
            camera: CameraPlan {
                mode: "follow_hero".to_string(),
                target_hero: primary.clone(),
                framing: "combat_medium".to_string(),
            },
        });
        if impact_end < final_source_end {
            beats.push(StoryBeat {
                kind: "result".to_string(),
                source_start_seconds: impact_end,
                source_end_seconds: final_source_end,
                playback_speed: 1.0,
                camera: CameraPlan {
                    mode: "free_overview".to_string(),
                    target_hero: primary.clone(),
                    framing: "aftermath_wide".to_string(),
                },
            });
        }

        cues.push(AudioCue {
            output_time_seconds: output_peak,
            role: "impact_hit".to_string(),
            intensity: (candidate.score / 200.0).clamp(0.35, 1.0),
        });
        if candidate
            .reasons
            .iter()
            .any(|reason| reason.contains("buyback"))
        {
            cues.push(AudioCue {
                output_time_seconds: output_peak,
                role: "reversal_stinger".to_string(),
                intensity: 0.9,
            });
        }

        segments.push(DirectorSegment {
            candidate_id: candidate.id.clone(),
            source_start_seconds: source_start,
            source_peak_seconds: candidate.peak_seconds,
            source_peak_tick: candidate.anchor_tick,
            source_end_seconds: final_source_end,
            output_start_seconds: output_start,
            output_end_seconds: output_end,
            primary_hero: primary,
            score: candidate.score,
            narration_hint: narration_hint(candidate),
            beats,
        });
        output_cursor = (output_end - TRANSITION_SECONDS).max(0.0);
    }

    DirectorDocument {
        schema_version: DIRECTOR_SCHEMA_VERSION.to_string(),
        source_sha256: highlights.source_sha256.clone(),
        template: template.to_string(),
        total_duration_seconds: segments
            .last()
            .map(|segment| segment.output_end_seconds)
            .unwrap_or_default(),
        transition_seconds: TRANSITION_SECONDS,
        segments,
        audio: AudioPlan {
            music_role: "original_comic_hype".to_string(),
            bpm_min: 130,
            bpm_max: 160,
            game_audio_duck_db: -2.0,
            music_duck_under_voice_db: -8.0,
            cues,
        },
    }
}

fn reserved_mechanical_story(
    highlights: &HighlightDocument,
    max_clips: usize,
) -> Vec<&HighlightCandidate> {
    if max_clips == 0 {
        return Vec::new();
    }

    let Some(anchor) = highlights
        .candidates
        .iter()
        .find(|candidate| candidate.kind == "mechanical_counterplay")
    else {
        return Vec::new();
    };
    let Some(evidence) = anchor.interaction.as_ref() else {
        return vec![anchor];
    };
    if evidence.occurrence_count <= 1 {
        return vec![anchor];
    }

    let mut story = highlights
        .candidates
        .iter()
        .filter(|candidate| {
            candidate.kind == "mechanical_counterplay"
                && candidate.primary_hero == anchor.primary_hero
                && candidate
                    .interaction
                    .as_ref()
                    .is_some_and(|candidate_evidence| {
                        candidate_evidence.pattern_id == evidence.pattern_id
                            && candidate_evidence.occurrence_count == evidence.occurrence_count
                    })
        })
        .collect::<Vec<_>>();
    story.sort_by_key(|candidate| {
        candidate
            .interaction
            .as_ref()
            .map(|candidate_evidence| candidate_evidence.occurrence_index)
            .unwrap_or(usize::MAX)
    });
    story.truncate(max_clips);
    story
}

fn estimated_total_duration(candidates: &[&HighlightCandidate]) -> f32 {
    let source_duration = candidates
        .iter()
        .map(|candidate| {
            let start = candidate
                .start_seconds
                .max(candidate.peak_seconds - MAX_SOURCE_PRE_PEAK_SECONDS);
            let end = candidate
                .end_seconds
                .min(candidate.peak_seconds + MAX_SOURCE_POST_PEAK_SECONDS);
            end - start
        })
        .sum::<f32>();
    let transitions = candidates.len().saturating_sub(1) as f32 * TRANSITION_SECONDS;
    source_duration - transitions
}

fn overlap_ratio(left: &HighlightCandidate, right: &HighlightCandidate) -> f32 {
    let overlap_start = left.start_seconds.max(right.start_seconds);
    let overlap_end = left.end_seconds.min(right.end_seconds);
    let overlap = (overlap_end - overlap_start).max(0.0);
    let shortest = (left.end_seconds - left.start_seconds)
        .min(right.end_seconds - right.start_seconds)
        .max(f32::EPSILON);
    overlap / shortest
}

fn narration_hint(candidate: &HighlightCandidate) -> String {
    if candidate
        .reasons
        .iter()
        .any(|reason| reason.contains("buyback"))
    {
        return "A buyback turns the fight back around.".to_string();
    }

    match candidate.kind.as_str() {
        "first_blood" => "The first mistake becomes first blood.".to_string(),
        "multikill" => "One opening collapses the entire fight.".to_string(),
        "team_fight" => "Both teams commit, but only one controls the finish.".to_string(),
        "roshan_reward" => "Roshan converts into the next map advantage.".to_string(),
        "objective" => "The fight immediately converts into an objective.".to_string(),
        "mechanical_counterplay" => {
            "A planted tree becomes the opening for a repeated item counter.".to_string()
        }
        _ => "A small opening becomes a decisive pickoff.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d2_highlights_core::{DetectorIdentity, HighlightCandidate, InteractionEvidence};

    fn candidate(id: &str, rank: usize, start: f32, peak: f32, end: f32) -> HighlightCandidate {
        HighlightCandidate {
            id: id.to_string(),
            rank,
            kind: "team_fight".to_string(),
            title: "fight".to_string(),
            score: 160.0,
            start_seconds: start,
            peak_seconds: peak,
            end_seconds: end,
            hero_deaths: 4,
            anchor_tick: (peak * 30.0) as u32,
            primary_hero: Some("npc_dota_hero_axe".to_string()),
            participants: vec!["npc_dota_hero_axe".to_string()],
            reasons: vec!["4 hero deaths".to_string()],
            interaction: None,
            kill_sequence: None,
        }
    }

    fn highlights(candidates: Vec<HighlightCandidate>) -> HighlightDocument {
        HighlightDocument {
            schema_version: "1.0".to_string(),
            source_sha256: "abc".to_string(),
            detector: DetectorIdentity {
                name: "test".to_string(),
                version: "1".to_string(),
            },
            candidates,
        }
    }

    fn mechanical_candidate(
        id: &str,
        occurrence_index: usize,
        occurrence_count: usize,
        start: f32,
    ) -> HighlightCandidate {
        let mut result = candidate(id, 10 + occurrence_index, start, start + 5.0, start + 9.0);
        result.kind = "mechanical_counterplay".to_string();
        result.score = 70.0;
        result.primary_hero = Some("npc_dota_hero_windrunner".to_string());
        result.interaction = Some(InteractionEvidence {
            pattern_id: "hoodwink_ground_acorn_quelling_blade".to_string(),
            occurrence_index,
            occurrence_count,
            trigger_name: "hoodwink_acorn_shot".to_string(),
            response_name: "item_quelling_blade".to_string(),
            response_delay_seconds: 1.0,
            related_action: None,
            verification: None,
        });
        result
    }

    #[test]
    fn produces_three_story_beats_and_audio_impact() {
        let plan = build_director_plan(
            &highlights(vec![candidate("hl-001", 1, 80.0, 100.0, 108.0)]),
            "comic_hype_v1",
            6,
            90.0,
        );

        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].beats.len(), 3);
        assert_eq!(plan.audio.cues[0].role, "impact_hit");
        assert_eq!(plan.total_duration_seconds, 28.0);
    }

    #[test]
    fn removes_heavily_overlapping_candidates() {
        let plan = build_director_plan(
            &highlights(vec![
                candidate("hl-001", 1, 80.0, 100.0, 108.0),
                candidate("hl-002", 2, 85.0, 102.0, 110.0),
            ]),
            "comic_hype_v1",
            6,
            90.0,
        );

        assert_eq!(plan.segments.len(), 1);
    }

    #[test]
    fn generic_director_does_not_mix_in_per_hero_kill_reels() {
        let mut hero_reel = candidate("hk-001", 1, 20.0, 24.0, 27.0);
        hero_reel.kind = "hero_kill_sequence".to_string();
        hero_reel.score = 999.0;
        let plan = build_director_plan(
            &highlights(vec![hero_reel, candidate("hl-001", 2, 80.0, 100.0, 108.0)]),
            "comic_hype_v1",
            6,
            90.0,
        );

        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].candidate_id, "hl-001");
    }

    #[test]
    fn reserves_room_for_a_mechanical_story_candidate() {
        let mut mechanic = candidate("hl-mechanic", 4, 300.0, 305.0, 309.0);
        mechanic.kind = "mechanical_counterplay".to_string();
        mechanic.score = 70.0;
        let plan = build_director_plan(
            &highlights(vec![
                candidate("hl-001", 1, 80.0, 100.0, 108.0),
                candidate("hl-002", 2, 140.0, 160.0, 168.0),
                candidate("hl-003", 3, 200.0, 220.0, 228.0),
                mechanic,
            ]),
            "comic_hype_v1",
            6,
            90.0,
        );

        assert!(
            plan.segments
                .iter()
                .any(|segment| segment.candidate_id == "hl-mechanic")
        );
    }

    #[test]
    fn reserves_every_occurrence_of_a_verified_repeated_interaction() {
        let plan = build_director_plan(
            &highlights(vec![
                candidate("hl-fight-1", 1, 80.0, 100.0, 108.0),
                candidate("hl-fight-2", 2, 140.0, 160.0, 168.0),
                candidate("hl-fight-3", 3, 200.0, 220.0, 228.0),
                mechanical_candidate("hl-tree-1", 1, 6, 300.0),
                mechanical_candidate("hl-tree-2", 2, 6, 330.0),
                mechanical_candidate("hl-tree-3", 3, 6, 400.0),
                mechanical_candidate("hl-tree-4", 4, 6, 430.0),
                mechanical_candidate("hl-tree-5", 5, 6, 460.0),
                mechanical_candidate("hl-tree-6", 6, 6, 500.0),
            ]),
            "comic_hype_v1",
            10,
            90.0,
        );
        let ids = plan
            .segments
            .iter()
            .map(|segment| segment.candidate_id.as_str())
            .collect::<Vec<_>>();

        assert!(ids.contains(&"hl-tree-1"));
        assert!(ids.contains(&"hl-tree-2"));
        assert!(ids.contains(&"hl-tree-3"));
        assert!(ids.contains(&"hl-tree-4"));
        assert!(ids.contains(&"hl-tree-5"));
        assert!(ids.contains(&"hl-tree-6"));
        assert_eq!(
            ids.iter()
                .filter(|candidate_id| candidate_id.starts_with("hl-tree-"))
                .count(),
            6
        );
    }

    #[test]
    fn orders_selected_segments_chronologically() {
        let plan = build_director_plan(
            &highlights(vec![
                candidate("hl-002", 1, 300.0, 320.0, 328.0),
                candidate("hl-001", 2, 80.0, 100.0, 108.0),
            ]),
            "comic_hype_v1",
            6,
            90.0,
        );

        assert_eq!(plan.segments[0].candidate_id, "hl-001");
        assert_eq!(plan.segments[1].candidate_id, "hl-002");
    }

    #[test]
    fn duration_budget_keeps_highest_ranked_candidate() {
        let plan = build_director_plan(
            &highlights(vec![
                candidate("hl-high", 1, 300.0, 320.0, 328.0),
                candidate("hl-low", 2, 80.0, 100.0, 108.0),
            ]),
            "comic_hype_v1",
            6,
            28.0,
        );

        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].candidate_id, "hl-high");
    }
}
