export type StageStatus = "pending" | "running" | "complete" | "failed";

export interface AnalysisProgress {
  stage: string;
  status: StageStatus;
  message: string;
}

export interface Capabilities {
  analysisReady: boolean;
  renderReady: boolean;
  ffmpegFound: boolean;
  ffprobeFound: boolean;
  dota2Found: boolean;
  renderReason: string | null;
  jobsRoot: string;
  recommendedReplayDirectory: string | null;
}

export interface ReplayLookupResult {
  replayDirectory: string;
  replayId: string;
  path: string;
}

export interface RecentJob {
  jobId: string;
  sourceName: string;
  sourcePath: string;
  byteLength: number;
  candidateCount: number;
  durationSeconds: number;
  createdUnixSeconds: number;
}

export interface DemSource {
  path: string;
  byte_length: number;
  sha256: string;
  magic: string;
}

export interface ReplayPlayer {
  slot: number;
  hero_name: string;
  game_team: number | null;
  is_fake_client: boolean;
}

export interface ReplayMetadata {
  playback_ticks: number;
  playback_time_seconds: number;
  game_build: number;
  match_id: number | null;
  game_mode: number | null;
  game_winner: number | null;
  players: ReplayPlayer[];
}

export interface HighlightCandidate {
  id: string;
  rank: number;
  kind: string;
  title: string;
  score: number;
  start_seconds: number;
  peak_seconds: number;
  end_seconds: number;
  hero_deaths: number;
  anchor_tick: number;
  primary_hero: string | null;
  participants: string[];
  reasons: string[];
  interaction?: {
    pattern_id: string;
    occurrence_index: number;
    occurrence_count: number;
    trigger_name: string;
    response_name: string;
    response_delay_seconds: number;
    related_action?: string;
    verification?: {
      method: string;
      tree_entity_handle: number;
      tree_created_tick: number;
      tree_deleted_tick: number;
      matched_tree_order_tick: number;
      first_fifteen_occurrence_index?: number;
      first_fifteen_occurrence_count: number;
      source_to_responder_salute_count: number;
    };
  };
  kill_sequence?: {
    hero: string;
    sequence_index: number;
    sequence_count: number;
    total_kills: number;
    kills: Array<{
      death_tick: number;
      death_time_seconds: number;
      target_hero: string;
      inflictor?: string;
      setup_tick: number;
      setup_time_seconds: number;
      setup_action?: string;
    }>;
  };
}

export interface HighlightDocument {
  schema_version: string;
  source_sha256: string;
  detector: {
    name: string;
    version: string;
  };
  candidates: HighlightCandidate[];
}

export type StoryCategory =
  | "comedy"
  | "skill"
  | "mistake"
  | "fight"
  | "objective";

export type StoryConfidenceLevel = "high" | "medium" | "low";

export type StoryArcBeatKind =
  | "setup"
  | "development"
  | "turn"
  | "payoff"
  | "reaction";

export type StoryCameraMode =
  | "player_perspective"
  | "hero_chase"
  | "directed";

export type StoryTakeRole = "primary" | "alternate";

export interface StoryEvidence {
  id: string;
  kind:
    | "trigger"
    | "response"
    | "verification"
    | "kill"
    | "reaction"
    | "context";
  candidate_id?: string;
  tick?: number;
  time_seconds: number;
  actor?: string;
  target?: string;
  action?: string;
  detail: string;
}

export interface StoryArcBeat {
  id: string;
  kind: StoryArcBeatKind;
  source_start_seconds: number;
  source_end_seconds: number;
  summary: string;
  evidence_ids: string[];
}

export interface StoryShot {
  id: string;
  order: number;
  take_group_id: string;
  take_role: StoryTakeRole;
  include_in_default_cut: boolean;
  beat_id: string;
  candidate_id: string;
  source_start_seconds: number;
  source_end_seconds: number;
  camera_mode: StoryCameraMode;
  target_hero?: string;
  framing: "normal_gameplay" | "close_action" | "combat_wide";
  purpose: string;
  fallback: {
    camera_mode: StoryCameraMode;
    target_hero?: string;
    reason: string;
  };
}

export interface HighlightStory {
  id: string;
  rank: number;
  category: StoryCategory;
  template_id: string;
  title: string;
  primary_hero: string;
  participants: Array<{
    hero: string;
    role: "protagonist" | "opponent" | "target" | "support";
  }>;
  candidate_ids: string[];
  source_start_seconds: number;
  source_peak_seconds: number;
  source_end_seconds: number;
  priority_score: number;
  confidence: {
    level: StoryConfidenceLevel;
    score: number;
    reasons: string[];
  };
  review_required: boolean;
  evidence: StoryEvidence[];
  beats: StoryArcBeat[];
  shots: StoryShot[];
  switch_windows: Array<{
    take_group_id: string;
    alternate_shot_id: string;
    source_start_seconds: number;
    source_end_seconds: number;
    reason: string;
  }>;
  reasons: string[];
}

export interface StoryDocument {
  schema_version: string;
  source_sha256: string;
  detector: {
    name: string;
    version: string;
  };
  stories: HighlightStory[];
}

export interface StoryBeat {
  kind: string;
  source_start_seconds: number;
  source_end_seconds: number;
  playback_speed: number;
  camera: {
    mode: string;
    target_hero: string | null;
    framing: string;
  };
}

export interface DirectorSegment {
  candidate_id: string;
  source_start_seconds: number;
  source_peak_seconds: number;
  source_peak_tick: number;
  source_end_seconds: number;
  output_start_seconds: number;
  output_end_seconds: number;
  primary_hero: string | null;
  score: number;
  narration_hint: string;
  beats: StoryBeat[];
}

export interface DirectorDocument {
  schema_version: string;
  source_sha256: string;
  template: string;
  total_duration_seconds: number;
  transition_seconds: number;
  segments: DirectorSegment[];
  audio: {
    music_role: string;
    bpm_min: number;
    bpm_max: number;
    game_audio_duck_db: number;
    music_duck_under_voice_db: number;
    cues: Array<{
      output_time_seconds: number;
      role: string;
      intensity: number;
    }>;
  };
}

export interface AnalysisSummary {
  job_id: string;
  job_dir: string;
  source: DemSource;
  replay: ReplayMetadata;
  event_count: number;
  highlights: HighlightDocument;
  stories: StoryDocument;
  director: DirectorDocument;
  reused_existing_job: boolean;
}

export interface EditPlanClip {
  clipId: string;
  candidateId: string;
  viewHero: string | null;
  cameraMode: ClipCameraMode;
  takeGroupId: string | null;
  takeRole: StoryTakeRole;
  includeInFinal: boolean;
  sourceStartSeconds: number;
  sourceEndSeconds: number;
}

export interface SaveEditPlanRequest {
  jobId: string;
  mode: "manual";
  clips: EditPlanClip[];
  settings: RenderSettings;
}

export interface SaveEditPlanResult {
  selectedClipCount: number;
  totalDurationSeconds: number;
}

export interface LoadedEditPlan {
  mode: "manual" | "automatic" | "review";
  clips: EditPlanClip[];
  settings: RenderSettings;
}

export type ClipCameraMode =
  | "directed"
  | "free_camera"
  | "hero_chase"
  | "player_perspective";

export type CameraStyle =
  | "auto_director"
  | "hero_focus"
  | "tactical_overview";

export type BgmMode = "original" | "custom" | "game_only";

export interface RenderSettings {
  cameraStyle: CameraStyle;
  cleanHud: boolean;
  slowMotion: boolean;
  replayEmphasis: boolean;
  bgmMode: BgmMode;
  customBgmPath: string | null;
  gameAudioVolume: number;
  bgmVolume: number;
  impactSfx: boolean;
  systemNarration: boolean;
}

export interface RenderProgress {
  stage: string;
  status: StageStatus;
  message: string;
  percent: number;
  currentClip: number;
  totalClips: number;
}

export interface RenderResult {
  outputPath: string;
  qcReportPath: string;
  sourceAssetsDir: string;
  sourceAssets: RenderSourceAsset[];
  durationSeconds: number;
  width: number;
  height: number;
  segmentCount: number;
  sourceAssetCount: number;
  warnings: string[];
}

export interface RenderSourceAsset {
  assetId: string;
  sceneNumber: number;
  takeIndex: number;
  clipId: string;
  takeGroupId: string | null;
  takeRole: StoryTakeRole;
  includedInFinal: boolean;
  outputPath: string;
  viewHero: string | null;
  cameraMode: ClipCameraMode;
  sourceStartSeconds: number;
  sourceEndSeconds: number;
}
