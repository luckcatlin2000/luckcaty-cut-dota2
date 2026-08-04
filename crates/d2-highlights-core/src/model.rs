use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MANIFEST_SCHEMA_VERSION: &str = "1.0";
pub const TIMELINE_SCHEMA_VERSION: &str = "1.2";
pub const HIGHLIGHT_SCHEMA_VERSION: &str = "1.4";
pub const DIRECTOR_SCHEMA_VERSION: &str = "1.4";
pub const STORY_SCHEMA_VERSION: &str = "1.2";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DemSource {
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
    pub magic: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Running,
    Complete,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StageRecord {
    pub status: StageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct JobManifest {
    pub schema_version: String,
    pub job_id: String,
    pub created_unix_seconds: u64,
    pub source: DemSource,
    pub stages: BTreeMap<String, StageRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ParserIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplayPlayer {
    pub slot: u8,
    pub hero_name: String,
    pub game_team: Option<i32>,
    pub is_fake_client: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ReplayMetadata {
    pub playback_ticks: i32,
    pub playback_time_seconds: f32,
    pub game_build: u32,
    pub match_id: Option<u64>,
    pub game_mode: Option<i32>,
    pub game_winner: Option<i32>,
    #[serde(default)]
    pub players: Vec<ReplayPlayer>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CombatEvent {
    pub tick: u32,
    pub time_seconds: Option<f32>,
    pub event_type: String,
    pub attacker: Option<String>,
    pub target: Option<String>,
    pub inflictor: Option<String>,
    pub damage_source: Option<String>,
    pub value: Option<u32>,
    pub health: Option<i32>,
    pub attacker_team: Option<u32>,
    pub target_team: Option<u32>,
    pub location_x: Option<f32>,
    pub location_y: Option<f32>,
    pub attacker_is_hero: Option<bool>,
    pub target_is_hero: Option<bool>,
    pub long_range_kill: Option<bool>,
    pub will_reincarnate: Option<bool>,
    pub assist_players: Vec<i32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TreeOrderEvent {
    pub tick: u32,
    pub player_entity_index: Option<i32>,
    pub unit_entity_indices: Vec<i32>,
    pub unit_class_names: Vec<String>,
    pub target_tree_index: Option<i32>,
    pub ability_entity_index: Option<i32>,
    pub sequence_number: Option<i32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TemporaryTreeState {
    Created,
    Deleted,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TemporaryTreeEvent {
    pub tick: u32,
    pub entity_index: u32,
    pub entity_handle: u32,
    pub state: TemporaryTreeState,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PlayerSaluteEvent {
    pub tick: u32,
    pub source_player_id: Option<i32>,
    pub target_player_id: Option<i32>,
    pub tip_amount: Option<u32>,
    pub event_id: Option<u32>,
    pub num_recent_tips: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TimelineDocument {
    pub schema_version: String,
    pub source_sha256: String,
    pub parser: ParserIdentity,
    pub replay: ReplayMetadata,
    pub events: Vec<CombatEvent>,
    #[serde(default)]
    pub tree_orders: Vec<TreeOrderEvent>,
    #[serde(default)]
    pub temporary_trees: Vec<TemporaryTreeEvent>,
    #[serde(default)]
    pub salutes: Vec<PlayerSaluteEvent>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DetectorIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InteractionEvidence {
    pub pattern_id: String,
    pub occurrence_index: usize,
    pub occurrence_count: usize,
    pub trigger_name: String,
    pub response_name: String,
    pub response_delay_seconds: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<InteractionVerification>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InteractionVerification {
    pub method: String,
    pub tree_entity_handle: u32,
    pub tree_created_tick: u32,
    pub tree_deleted_tick: u32,
    pub matched_tree_order_tick: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_fifteen_occurrence_index: Option<usize>,
    pub first_fifteen_occurrence_count: usize,
    pub source_to_responder_salute_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HeroKillMoment {
    pub death_tick: u32,
    pub death_time_seconds: f32,
    pub target_hero: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inflictor: Option<String>,
    pub setup_tick: u32,
    pub setup_time_seconds: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_action: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HeroKillSequenceEvidence {
    pub hero: String,
    pub sequence_index: usize,
    pub sequence_count: usize,
    pub total_kills: usize,
    pub kills: Vec<HeroKillMoment>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HighlightCandidate {
    pub id: String,
    pub rank: usize,
    pub kind: String,
    pub title: String,
    pub score: f32,
    pub start_seconds: f32,
    pub peak_seconds: f32,
    pub end_seconds: f32,
    pub hero_deaths: usize,
    pub anchor_tick: u32,
    pub primary_hero: Option<String>,
    pub participants: Vec<String>,
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction: Option<InteractionEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kill_sequence: Option<HeroKillSequenceEvidence>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HighlightDocument {
    pub schema_version: String,
    pub source_sha256: String,
    pub detector: DetectorIdentity,
    pub candidates: Vec<HighlightCandidate>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoryCategory {
    Comedy,
    Skill,
    Mistake,
    Fight,
    Objective,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoryConfidenceLevel {
    High,
    Medium,
    Low,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StoryConfidence {
    pub level: StoryConfidenceLevel,
    pub score: f32,
    pub reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoryParticipantRole {
    Protagonist,
    Opponent,
    Target,
    Support,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StoryParticipant {
    pub hero: String,
    pub role: StoryParticipantRole,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoryEvidenceKind {
    Trigger,
    Response,
    Verification,
    Kill,
    Reaction,
    Context,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StoryEvidence {
    pub id: String,
    pub kind: StoryEvidenceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<u32>,
    pub time_seconds: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoryArcBeatKind {
    Setup,
    Development,
    Turn,
    Payoff,
    Reaction,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StoryArcBeat {
    pub id: String,
    pub kind: StoryArcBeatKind,
    pub source_start_seconds: f32,
    pub source_end_seconds: f32,
    pub summary: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoryCameraMode {
    PlayerPerspective,
    HeroChase,
    Directed,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoryFraming {
    NormalGameplay,
    CloseAction,
    CombatWide,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoryTakeRole {
    Primary,
    Alternate,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StoryShotFallback {
    pub camera_mode: StoryCameraMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_hero: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StoryShot {
    pub id: String,
    pub order: usize,
    pub take_group_id: String,
    pub take_role: StoryTakeRole,
    pub include_in_default_cut: bool,
    pub beat_id: String,
    pub candidate_id: String,
    pub source_start_seconds: f32,
    pub source_end_seconds: f32,
    pub camera_mode: StoryCameraMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_hero: Option<String>,
    pub framing: StoryFraming,
    pub purpose: String,
    pub fallback: StoryShotFallback,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StorySwitchWindow {
    pub take_group_id: String,
    pub alternate_shot_id: String,
    pub source_start_seconds: f32,
    pub source_end_seconds: f32,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HighlightStory {
    pub id: String,
    pub rank: usize,
    pub category: StoryCategory,
    pub template_id: String,
    pub title: String,
    pub primary_hero: String,
    pub participants: Vec<StoryParticipant>,
    pub candidate_ids: Vec<String>,
    pub source_start_seconds: f32,
    pub source_peak_seconds: f32,
    pub source_end_seconds: f32,
    pub priority_score: f32,
    pub confidence: StoryConfidence,
    pub review_required: bool,
    pub evidence: Vec<StoryEvidence>,
    pub beats: Vec<StoryArcBeat>,
    pub shots: Vec<StoryShot>,
    pub switch_windows: Vec<StorySwitchWindow>,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StoryDocument {
    pub schema_version: String,
    pub source_sha256: String,
    pub detector: DetectorIdentity,
    pub stories: Vec<HighlightStory>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CameraPlan {
    pub mode: String,
    pub target_hero: Option<String>,
    pub framing: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StoryBeat {
    pub kind: String,
    pub source_start_seconds: f32,
    pub source_end_seconds: f32,
    pub playback_speed: f32,
    pub camera: CameraPlan,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DirectorSegment {
    pub candidate_id: String,
    pub source_start_seconds: f32,
    pub source_peak_seconds: f32,
    pub source_peak_tick: u32,
    pub source_end_seconds: f32,
    pub output_start_seconds: f32,
    pub output_end_seconds: f32,
    pub primary_hero: Option<String>,
    pub score: f32,
    pub narration_hint: String,
    pub beats: Vec<StoryBeat>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AudioCue {
    pub output_time_seconds: f32,
    pub role: String,
    pub intensity: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AudioPlan {
    pub music_role: String,
    pub bpm_min: u32,
    pub bpm_max: u32,
    pub game_audio_duck_db: f32,
    pub music_duck_under_voice_db: f32,
    pub cues: Vec<AudioCue>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DirectorDocument {
    pub schema_version: String,
    pub source_sha256: String,
    pub template: String,
    pub total_duration_seconds: f32,
    pub transition_seconds: f32,
    pub segments: Vec<DirectorSegment>,
    pub audio: AudioPlan,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_json_preserves_match_outcome_contract() {
        let timeline = TimelineDocument {
            schema_version: TIMELINE_SCHEMA_VERSION.to_string(),
            source_sha256: "abc".to_string(),
            parser: ParserIdentity {
                name: "fixture".to_string(),
                version: "1".to_string(),
            },
            replay: ReplayMetadata {
                playback_ticks: 30,
                playback_time_seconds: 1.0,
                game_build: 42,
                match_id: Some(123),
                game_mode: Some(22),
                game_winner: Some(2),
                players: vec![ReplayPlayer {
                    slot: 0,
                    hero_name: "npc_dota_hero_mirana".to_string(),
                    game_team: Some(2),
                    is_fake_client: false,
                }],
            },
            events: Vec::new(),
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
        };

        let json = serde_json::to_vec(&timeline).unwrap();
        let decoded: TimelineDocument = serde_json::from_slice(&json).unwrap();

        assert_eq!(decoded.replay.match_id, Some(123));
        assert_eq!(decoded.replay.game_winner, Some(2));
        assert_eq!(decoded.replay.players[0].slot, 0);
        assert_eq!(decoded.replay.players[0].hero_name, "npc_dota_hero_mirana");
    }

    #[test]
    fn old_replay_metadata_without_players_remains_readable() {
        let json = br#"{
            "playback_ticks": 30,
            "playback_time_seconds": 1.0,
            "game_build": 42,
            "match_id": null,
            "game_mode": null,
            "game_winner": null
        }"#;
        let decoded: ReplayMetadata = serde_json::from_slice(json).unwrap();

        assert!(decoded.players.is_empty());
    }
}
