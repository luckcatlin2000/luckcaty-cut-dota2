mod activity;
mod cache_cleanup;
mod diagnostics;
mod external_modules;
mod replay_library;
mod timestamp_import;
mod video_output;
mod wtf;

use activity::{Activities, Activity};

use d2_highlights_core::{HighlightDocument, JobManifest, TimelineDocument};
use d2_highlights_pipeline::{AnalysisProgress, AnalysisSummary, analyze_highlights_with_progress};
use d2_highlights_renderer::{
    BgmMode, CameraStyle, CancellationToken, ClipCameraMode, RENDER_SCHEMA_VERSION, RenderClip,
    RenderProgress, RenderRequest, RenderResult, RenderResumeSummary, RenderSettings,
    RenderSourceAsset, RenderTakeRole, inspect_resume, render,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

const EDIT_PLAN_SCHEMA_VERSION: &str = "d2h.edit-plan/1.6";
const DUAL_EDIT_PLAN_SCHEMA_VERSIONS: [&str; 1] = ["d2h.edit-plan/1.5"];
const LEGACY_EDIT_PLAN_SCHEMA_VERSIONS: [&str; 2] = ["d2h.edit-plan/1.4", "d2h.edit-plan/1.3"];
const DOTA2_SETTINGS_SCHEMA_VERSION: &str = "d2h.dota2-path/1.0";
const DOTA2_SETTINGS_FILE: &str = "dota2-path.json";

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DesktopModule {
    Highlights,
    Wtf,
}

#[tauri::command]
fn get_desktop_module(module: State<'_, DesktopModule>) -> DesktopModule {
    *module
}

const fn default_true() -> bool {
    true
}

const fn default_story_pre_roll_seconds() -> f32 {
    60.0
}

#[derive(Clone)]
struct AppState {
    jobs_root: PathBuf,
    dota2_runtime: Arc<Mutex<Dota2Runtime>>,
    dota2_settings_path: PathBuf,
    video_output: video_output::Settings,
    ffmpeg_exe: Option<PathBuf>,
    ffprobe_exe: Option<PathBuf>,
    render_runtime: Arc<Mutex<RenderRuntime>>,
    activities: Activities,
}

#[derive(Clone)]
struct Dota2Runtime {
    executable: Option<PathBuf>,
    source: Dota2PathSource,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum Dota2PathSource {
    Automatic,
    Custom,
    Missing,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Dota2PathSettings {
    schema_version: String,
    executable_path: String,
}

#[derive(Default)]
struct RenderRuntime {
    active: bool,
    cancellation: Option<CancellationToken>,
}

#[derive(Default)]
struct PendingUpdate(Mutex<UpdateRuntime>);

#[derive(Default)]
struct UpdateRuntime {
    pending: Option<Update>,
    installing: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppUpdateMetadata {
    version: String,
    current_version: String,
    notes: String,
    published_at: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum AppUpdateEvent {
    Started { content_length: Option<u64> },
    Progress { chunk_length: usize },
    Finished,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    analysis_ready: bool,
    render_ready: bool,
    ffmpeg_found: bool,
    ffprobe_found: bool,
    dota2_found: bool,
    render_reason: Option<String>,
    jobs_root: String,
    recommended_replay_directory: Option<String>,
    dota2_path: Option<String>,
    dota2_path_source: Dota2PathSource,
    video_output_directory: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecentJob {
    job_id: String,
    source_name: String,
    source_path: String,
    byte_length: u64,
    candidate_count: usize,
    duration_seconds: f32,
    created_unix_seconds: u64,
    last_opened_unix_seconds: u64,
}

#[derive(Deserialize, Serialize)]
struct RecentJobActivity {
    last_opened_unix_seconds: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayLookupResult {
    replay_directory: String,
    replay_id: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveEditPlanRequest {
    job_id: String,
    active_mode: EditPlanMode,
    plans: Vec<EditPlanSlotInput>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EditPlanMode {
    #[default]
    Default,
    Wtf,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClipDurationMode {
    #[default]
    Short,
    Story,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditPlanSlotInput {
    mode: EditPlanMode,
    #[serde(default)]
    selected_hero: Option<String>,
    #[serde(default)]
    highlight_rule_ids: Vec<String>,
    #[serde(default)]
    selected_clip_id: Option<String>,
    #[serde(default)]
    source_story_id: Option<String>,
    #[serde(default)]
    clips: Vec<EditPlanClipInput>,
    #[serde(default)]
    settings: RenderSettings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditPlanClipInput {
    clip_id: String,
    candidate_id: String,
    #[serde(default)]
    imported: Option<timestamp_import::ImportedClip>,
    view_hero: Option<String>,
    #[serde(default)]
    camera_mode: ClipCameraMode,
    #[serde(default)]
    take_group_id: Option<String>,
    #[serde(default)]
    take_role: RenderTakeRole,
    #[serde(default = "default_true")]
    include_in_final: bool,
    #[serde(default)]
    duration_mode: ClipDurationMode,
    #[serde(default = "default_story_pre_roll_seconds")]
    story_pre_roll_seconds: f32,
    short_source_start_seconds: f32,
    short_source_end_seconds: f32,
    source_start_seconds: f32,
    source_end_seconds: f32,
}

#[derive(Deserialize, Serialize)]
struct EditPlanDocument {
    schema_version: String,
    source_sha256: String,
    job_id: String,
    #[serde(default)]
    active_mode: EditPlanMode,
    #[serde(default)]
    plans: Vec<EditPlanSlot>,
    #[serde(default)]
    mode: String,
    updated_unix_seconds: u64,
    #[serde(default)]
    clips: Vec<EditPlanClip>,
    #[serde(default)]
    settings: RenderSettings,
}

#[derive(Clone, Deserialize, Serialize)]
struct EditPlanClip {
    #[serde(default)]
    clip_id: String,
    candidate_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    imported: Option<timestamp_import::ImportedClip>,
    view_hero: Option<String>,
    #[serde(default)]
    camera_mode: ClipCameraMode,
    #[serde(default)]
    take_group_id: Option<String>,
    #[serde(default)]
    take_role: RenderTakeRole,
    #[serde(default = "default_true")]
    include_in_final: bool,
    #[serde(default)]
    duration_mode: ClipDurationMode,
    #[serde(default = "default_story_pre_roll_seconds")]
    story_pre_roll_seconds: f32,
    #[serde(default)]
    short_source_start_seconds: Option<f32>,
    #[serde(default)]
    short_source_end_seconds: Option<f32>,
    source_start_seconds: f32,
    source_end_seconds: f32,
}

#[derive(Clone, Deserialize, Serialize)]
struct EditPlanSlot {
    mode: EditPlanMode,
    #[serde(default)]
    selected_hero: Option<String>,
    #[serde(default)]
    highlight_rule_ids: Vec<String>,
    #[serde(default)]
    selected_clip_id: Option<String>,
    #[serde(default)]
    source_story_id: Option<String>,
    #[serde(default)]
    clips: Vec<EditPlanClip>,
    #[serde(default)]
    settings: RenderSettings,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveEditPlanResult {
    selected_clip_count: usize,
    total_duration_seconds: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadedEditPlan {
    active_mode: EditPlanMode,
    plans: Vec<LoadedEditPlanSlot>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadedEditPlanSlot {
    mode: EditPlanMode,
    selected_hero: Option<String>,
    highlight_rule_ids: Vec<String>,
    selected_clip_id: Option<String>,
    source_story_id: Option<String>,
    clips: Vec<LoadedEditPlanClip>,
    settings: RenderSettings,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadedEditPlanClip {
    clip_id: String,
    candidate_id: String,
    imported: Option<timestamp_import::ImportedClip>,
    view_hero: Option<String>,
    camera_mode: ClipCameraMode,
    take_group_id: Option<String>,
    take_role: RenderTakeRole,
    include_in_final: bool,
    duration_mode: ClipDurationMode,
    story_pre_roll_seconds: f32,
    short_source_start_seconds: f32,
    short_source_end_seconds: f32,
    source_start_seconds: f32,
    source_end_seconds: f32,
}

#[derive(Deserialize)]
struct StoredQcReport {
    schema_version: String,
    output_path: String,
    duration_seconds: f32,
    width: u32,
    height: u32,
    #[serde(default)]
    final_segment_count: usize,
    #[serde(default)]
    source_assets_dir: String,
    #[serde(default)]
    source_assets: Vec<RenderSourceAsset>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[tauri::command]
fn get_capabilities(state: State<'_, AppState>) -> Capabilities {
    capabilities_for_state(&state)
}

#[tauri::command]
fn set_dota2_executable(
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<Capabilities, String> {
    let runtime = match path {
        Some(path) => {
            let executable = validate_dota2_executable(Path::new(path.trim().trim_matches('"')))?;
            persist_dota2_path(&state.dota2_settings_path, Some(&executable))?;
            Dota2Runtime {
                executable: Some(executable),
                source: Dota2PathSource::Custom,
            }
        }
        None => {
            persist_dota2_path(&state.dota2_settings_path, None)?;
            automatic_dota2_runtime()
        }
    };
    *state
        .dota2_runtime
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = runtime;
    Ok(capabilities_for_state(&state))
}

fn capabilities_for_state(state: &AppState) -> Capabilities {
    let ffmpeg_found = state.ffmpeg_exe.is_some();
    let ffprobe_found = state.ffprobe_exe.is_some();
    let dota2_runtime = state
        .dota2_runtime
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let dota2_found = dota2_runtime.executable.is_some();
    let render_ready = ffmpeg_found && ffprobe_found && dota2_found;
    let render_reason = if render_ready {
        None
    } else if !dota2_found {
        Some("未找到 Dota 2。分析可用，生成真实游戏画面需要安装客户端。".to_string())
    } else if !ffmpeg_found || !ffprobe_found {
        Some("媒体工具不完整，请使用完整版目录或重新安装。".to_string())
    } else {
        Some("成片环境尚未就绪。".to_string())
    };
    Capabilities {
        analysis_ready: true,
        render_ready,
        ffmpeg_found,
        ffprobe_found,
        dota2_found,
        render_reason,
        jobs_root: state.jobs_root.display().to_string(),
        recommended_replay_directory: dota2_runtime
            .executable
            .as_deref()
            .and_then(replay_directory_from_dota2)
            .map(|path| path.display().to_string()),
        dota2_path: dota2_runtime
            .executable
            .map(|path| path.display().to_string()),
        dota2_path_source: dota2_runtime.source,
        video_output_directory: state.video_output.current(),
    }
}

#[tauri::command]
fn resolve_replay_by_id(
    replay_directory: String,
    replay_id: String,
) -> Result<ReplayLookupResult, String> {
    let replay_id = replay_id.trim();
    if !valid_replay_id(replay_id) {
        return Err("录像编号只能包含 1 到 20 位数字。".to_string());
    }

    let replay_directory = PathBuf::from(replay_directory.trim());
    if !replay_directory.is_absolute() {
        return Err("请输入完整的录像目录，或使用文件夹按钮选择。".to_string());
    }
    if !replay_directory.is_dir() {
        return Err("录像目录不存在，请重新选择。".to_string());
    }

    let replay_path = replay_directory.join(format!("{replay_id}.dem"));
    if !replay_path.is_file() {
        return Err(format!(
            "在当前目录中没有找到 {replay_id}.dem，请检查编号或录像目录。"
        ));
    }

    let mut file = fs::File::open(&replay_path)
        .map_err(|error| format!("无法读取 {}：{error}", replay_path.display()))?;
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic)
        .map_err(|_| format!("{replay_id}.dem 不是完整的 Source 2 录像。"))?;
    if &magic != b"PBDEMS2\0" {
        return Err(format!("{replay_id}.dem 不是有效的 Dota 2 Source 2 录像。"));
    }

    Ok(ReplayLookupResult {
        replay_directory: replay_directory.display().to_string(),
        replay_id: replay_id.to_string(),
        path: replay_path.display().to_string(),
    })
}

#[tauri::command]
fn get_recent_jobs(state: State<'_, AppState>) -> Result<Vec<RecentJob>, String> {
    let mut jobs = Vec::new();
    let entries = match fs::read_dir(&state.jobs_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(jobs),
        Err(error) => return Err(error.to_string()),
    };

    for entry in entries.flatten() {
        let job_dir = entry.path();
        let manifest_path = job_dir.join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        let Ok(manifest) = read_json::<JobManifest>(&manifest_path) else {
            continue;
        };
        let highlights =
            read_json::<HighlightDocument>(&job_dir.join("timeline").join("highlights.json")).ok();
        let duration_seconds = highlights
            .as_ref()
            .map(|document| {
                document
                    .candidates
                    .iter()
                    .filter(|candidate| candidate.kind == "hero_kill_sequence")
                    .map(|candidate| candidate.end_seconds - candidate.start_seconds)
                    .sum()
            })
            .unwrap_or_default();
        let last_opened_unix_seconds =
            read_json::<RecentJobActivity>(&job_dir.join("recent-activity.json"))
                .map(|activity| activity.last_opened_unix_seconds)
                .unwrap_or(manifest.created_unix_seconds);
        jobs.push(RecentJob {
            job_id: manifest.job_id,
            source_name: source_name(&manifest.source.path),
            source_path: manifest.source.path,
            byte_length: manifest.source.byte_length,
            candidate_count: highlights
                .as_ref()
                .map(|document| {
                    document
                        .candidates
                        .iter()
                        .filter(|candidate| candidate.kind == "hero_kill_sequence")
                        .count()
                })
                .unwrap_or_default(),
            duration_seconds,
            created_unix_seconds: manifest.created_unix_seconds,
            last_opened_unix_seconds,
        });
    }

    jobs.sort_by_key(|job| {
        std::cmp::Reverse((job.last_opened_unix_seconds, job.created_unix_seconds))
    });
    jobs.truncate(12);
    Ok(jobs)
}

#[tauri::command]
async fn preview_timestamp_plan(
    request: timestamp_import::PreviewRequest,
    state: State<'_, AppState>,
) -> Result<timestamp_import::Preview, String> {
    if !valid_job_id(&request.job_id) {
        return Err("任务编号无效。".into());
    }
    let lease = state.activities.begin(Activity::Export)?;
    let root = state.jobs_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let job = root.join(&request.job_id);
        let manifest: JobManifest =
            read_json(&job.join("manifest.json")).map_err(|_| "请先载入录像。")?;
        let timeline: TimelineDocument = read_json(&job.join("timeline/combat-events.json"))
            .map_err(|_| "本局分析数据不完整。")?;
        let (text, name) = match (request.path, request.text) {
            (Some(path), None) => (
                timestamp_import::read_text(Path::new(&path))?,
                source_name(&path),
            ),
            (None, Some(text)) => (text, "粘贴的时间表".into()),
            _ => return Err("请选择一个文件，或粘贴一整块时间表。".into()),
        };
        timestamp_import::build_preview(
            &text,
            &name,
            &manifest.source,
            &timeline,
            request.hero.as_deref(),
            request.time_basis.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn analyze_dem(
    dem_path: String,
    reset_edit_plan: bool,
    on_progress: Channel<AnalysisProgress>,
    state: State<'_, AppState>,
) -> Result<AnalysisSummary, String> {
    let lease = state.activities.begin(Activity::Analysis)?;
    let dem_path = PathBuf::from(dem_path);
    let jobs_root = state.jobs_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let summary = analyze_highlights_with_progress(&dem_path, &jobs_root, |progress| {
            let _ = on_progress.send(progress);
        })
        .map_err(|error| error.to_string())?;
        if reset_edit_plan {
            archive_edit_plan_for_reanalysis(&jobs_root, &summary.job_id)?;
        }
        let _ = mark_job_recent(&jobs_root, &summary.job_id);
        Ok(summary)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn archive_edit_plan_for_reanalysis(
    jobs_root: &Path,
    job_id: &str,
) -> Result<Option<PathBuf>, String> {
    if !valid_job_id(job_id) {
        return Err("任务编号无效，请重新打开分析结果。".to_string());
    }
    let director_dir = jobs_root.join(job_id).join("director");
    let source = director_dir.join("edit-plan.json");
    if !source.is_file() {
        return Ok(None);
    }
    let mut preserved: serde_json::Value = read_json(&source)?;
    let wtf = preserved_wtf_slot(&preserved)?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let mut suffix = 0_u32;
    let backup = loop {
        let file_name = if suffix == 0 {
            format!("edit-plan.before-reanalysis-{timestamp}.json")
        } else {
            format!("edit-plan.before-reanalysis-{timestamp}-{suffix}.json")
        };
        let candidate = director_dir.join(file_name);
        if !candidate.exists() {
            break candidate;
        }
        suffix += 1;
    };
    if let Some(wtf) = wtf {
        fs::copy(&source, &backup).map_err(|error| format!("备份当前剪辑方案失败：{error}"))?;
        preserved["plans"] = serde_json::json!([wtf]);
        preserved["active_mode"] = serde_json::json!("default");
        preserved["clips"] = serde_json::json!([]);
        preserved["settings"] =
            serde_json::to_value(RenderSettings::default()).map_err(|error| error.to_string())?;
        d2_highlights_core::write_json_pretty(&source, &preserved)
            .map_err(|error| format!("保存旧方案保留副本失败：{error}"))?;
    } else {
        fs::rename(&source, &backup).map_err(|error| format!("备份当前剪辑方案失败：{error}"))?;
    }
    Ok(Some(backup))
}

#[tauri::command]
fn save_edit_plan(
    request: SaveEditPlanRequest,
    state: State<'_, AppState>,
) -> Result<SaveEditPlanResult, String> {
    if !valid_job_id(&request.job_id) {
        return Err("任务编号无效，请重新打开分析结果。".to_string());
    }
    let _lease = state.activities.begin(Activity::Save)?;
    save_edit_plan_to(&state.jobs_root, request)
}

fn save_edit_plan_to(
    jobs_root: &Path,
    request: SaveEditPlanRequest,
) -> Result<SaveEditPlanResult, String> {
    if !valid_job_id(&request.job_id) {
        return Err("任务编号无效。".into());
    }
    if request.active_mode != EditPlanMode::Default
        || request.plans.len() != 1
        || request.plans[0].mode != EditPlanMode::Default
    {
        return Err("剪辑助手只保存高光方案，旧的其他方案会保留。".to_string());
    }

    let job_dir = jobs_root.join(&request.job_id);
    let manifest = read_json::<JobManifest>(&job_dir.join("manifest.json"))
        .map_err(|_| "任务数据不存在，请重新分析录像。".to_string())?;
    let highlights =
        read_json::<HighlightDocument>(&job_dir.join("timeline").join("highlights.json"))
            .map_err(|_| "高光候选数据不完整，请重新分析录像。".to_string())?;
    let timeline =
        read_json::<TimelineDocument>(&job_dir.join("timeline").join("combat-events.json"))
            .map_err(|_| "回放阵容数据不完整，请重新分析录像。".to_string())?;

    let output_path = job_dir.join("director/edit-plan.json");
    let preserved_wtf = if output_path.is_file() {
        let previous: serde_json::Value = read_json(&output_path)?;
        if previous["job_id"] != request.job_id
            || previous["source_sha256"] != manifest.source.sha256
        {
            return Err("已保存草稿与本局录像不一致，原文件已保留。".into());
        }
        preserved_wtf_slot(&previous)?
    } else {
        None
    };
    let (plan, active_duration_seconds) = build_edit_plan_slot(
        request
            .plans
            .into_iter()
            .next()
            .expect("one highlight plan"),
        &highlights,
        &timeline,
    )?;
    let active_clips = plan.clips.clone();
    let active_settings = plan.settings.clone();
    let selected_clip_count = active_clips.len();

    let document = EditPlanDocument {
        schema_version: EDIT_PLAN_SCHEMA_VERSION.to_string(),
        source_sha256: manifest.source.sha256,
        job_id: request.job_id,
        active_mode: request.active_mode,
        plans: vec![plan],
        mode: "highlight".to_string(),
        updated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs(),
        clips: active_clips,
        settings: active_settings,
    };
    let director_dir = job_dir.join("director");
    fs::create_dir_all(&director_dir).map_err(|error| error.to_string())?;
    let mut document = serde_json::to_value(document).map_err(|error| error.to_string())?;
    if let Some(preserved) = preserved_wtf {
        // Keep the old slot opaque: unknown future fields and stale candidates belong to its backup.
        document["plans"].as_array_mut().unwrap().push(preserved);
    }
    d2_highlights_core::write_json_pretty(&output_path, &document)
        .map_err(|error| format!("保存剪辑方案失败：{error}"))?;

    Ok(SaveEditPlanResult {
        selected_clip_count,
        total_duration_seconds: active_duration_seconds,
    })
}

fn preserved_wtf_slot(document: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
    let schema = document["schema_version"].as_str().unwrap_or_default();
    if !is_dual_edit_plan_schema(schema) && !LEGACY_EDIT_PLAN_SCHEMA_VERSIONS.contains(&schema) {
        return Err("已保存草稿的版本不受支持，原文件已保留，请勿覆盖。".into());
    }
    let slots: Vec<_> = document["plans"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|slot| slot["mode"] == "wtf")
        .collect();
    if slots.len() > 1 {
        return Err("已保存草稿包含重复的旧方案，原文件已保留。".into());
    }
    Ok(slots.first().map(|slot| (*slot).clone()))
}

fn build_edit_plan_slot(
    input: EditPlanSlotInput,
    highlights: &HighlightDocument,
    timeline: &TimelineDocument,
) -> Result<(EditPlanSlot, f32), String> {
    if let Some(hero) = input.selected_hero.as_deref()
        && !timeline.replay.players.is_empty()
        && !timeline
            .replay
            .players
            .iter()
            .any(|player| player.hero_name == hero)
    {
        return Err("当前方案选择的主角不在本局阵容中。".to_string());
    }
    if input.highlight_rule_ids.iter().any(|rule_id| {
        rule_id.is_empty()
            || rule_id.len() > 64
            || !rule_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }) {
        return Err("当前方案包含无效的高光规则。".to_string());
    }

    let mut seen = HashSet::new();
    let mut clips = Vec::with_capacity(input.clips.len());
    let mut total_duration_seconds = 0.0;
    for clip in input.clips {
        if !valid_clip_id(&clip.clip_id) {
            return Err("剪辑方案包含无效的片段编号。".to_string());
        }
        if !seen.insert(clip.clip_id.clone()) {
            return Err("剪辑方案包含重复的片段编号。".to_string());
        }
        let candidate = highlights
            .candidates
            .iter()
            .find(|candidate| candidate.id == clip.candidate_id);
        if let Some(imported) = &clip.imported {
            timestamp_import::validate_anchor(imported, &clip.candidate_id, timeline)?;
        } else if candidate.is_none() {
            return Err(format!("找不到候选片段 {}。", clip.candidate_id));
        }
        if !clip.source_start_seconds.is_finite()
            || !clip.source_end_seconds.is_finite()
            || clip.source_start_seconds < 0.0
            || clip.source_end_seconds <= clip.source_start_seconds
            || clip.source_end_seconds > timeline.replay.playback_time_seconds
        {
            return Err(format!("候选 {} 的起止时间无效。", clip.candidate_id));
        }
        if !valid_frame_duration(clip.source_start_seconds, clip.source_end_seconds) {
            return Err(format!(
                "片段 {} 的时长必须在 1 到 100 秒之间。",
                clip.clip_id
            ));
        }
        if !clip.story_pre_roll_seconds.is_finite()
            || !(0.0..=90.0).contains(&clip.story_pre_roll_seconds)
        {
            return Err(format!(
                "片段 {} 的剧情前置时长必须在 0 到 90 秒之间。",
                clip.clip_id
            ));
        }
        if !clip.short_source_start_seconds.is_finite()
            || !clip.short_source_end_seconds.is_finite()
            || clip.short_source_start_seconds < 0.0
            || clip.short_source_end_seconds <= clip.short_source_start_seconds
            || clip.short_source_end_seconds > timeline.replay.playback_time_seconds
            || !valid_frame_duration(
                clip.short_source_start_seconds,
                clip.short_source_end_seconds,
            )
        {
            return Err(format!("片段 {} 保存的短击杀时间无效。", clip.clip_id));
        }
        if let Some(hero) = clip.view_hero.as_deref() {
            let valid_hero = if timeline.replay.players.is_empty() {
                candidate.is_some_and(|c| {
                    c.participants.iter().any(|value| value == hero)
                        || c.primary_hero.as_deref() == Some(hero)
                })
            } else {
                timeline
                    .replay
                    .players
                    .iter()
                    .any(|player| player.hero_name == hero)
            };
            if !valid_hero {
                return Err(format!(
                    "候选 {} 所选英雄不在本局阵容中。",
                    clip.candidate_id
                ));
            }
        }

        if clip.include_in_final {
            total_duration_seconds += clip.source_end_seconds - clip.source_start_seconds;
        }
        clips.push(EditPlanClip {
            clip_id: clip.clip_id,
            candidate_id: clip.candidate_id,
            imported: clip.imported,
            view_hero: clip.view_hero,
            camera_mode: normalize_user_camera_mode(clip.camera_mode),
            take_group_id: clip.take_group_id,
            take_role: clip.take_role,
            include_in_final: clip.include_in_final,
            duration_mode: clip.duration_mode,
            story_pre_roll_seconds: clip.story_pre_roll_seconds,
            short_source_start_seconds: Some(clip.short_source_start_seconds),
            short_source_end_seconds: Some(clip.short_source_end_seconds),
            source_start_seconds: clip.source_start_seconds,
            source_end_seconds: clip.source_end_seconds,
        });
    }
    if !clips.is_empty() {
        validate_edit_plan_take_groups(&clips)?;
    }
    if let Some(selected_clip_id) = input.selected_clip_id.as_deref()
        && !clips.iter().any(|clip| clip.clip_id == selected_clip_id)
    {
        return Err("当前方案选中的片段已经不存在。".to_string());
    }

    Ok((
        EditPlanSlot {
            mode: input.mode,
            selected_hero: input.selected_hero,
            highlight_rule_ids: input.highlight_rule_ids,
            selected_clip_id: input.selected_clip_id,
            source_story_id: input.source_story_id,
            clips,
            settings: manual_render_settings(input.settings),
        },
        total_duration_seconds,
    ))
}

fn validate_edit_plan_take_groups(clips: &[EditPlanClip]) -> Result<(), String> {
    let mut groups = HashMap::<String, Vec<&EditPlanClip>>::new();
    for clip in clips {
        if let Some(group_id) = clip.take_group_id.as_deref() {
            if group_id.is_empty()
                || group_id.len() > 160
                || !group_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(format!(
                    "素材 {} 的场次编号无效，请重新采用导演方案。",
                    clip.clip_id
                ));
            }
            groups.entry(group_id.to_string()).or_default().push(clip);
        } else if clip.take_role != RenderTakeRole::Primary || !clip.include_in_final {
            return Err(format!(
                "独立素材 {} 必须作为默认入片的主机位。",
                clip.clip_id
            ));
        }
    }
    if !clips.iter().any(|clip| clip.include_in_final) {
        return Err("请至少保留一个默认入片的主机位素材。".to_string());
    }
    for (group_id, group) in groups {
        let primaries = group
            .iter()
            .copied()
            .filter(|clip| clip.take_role == RenderTakeRole::Primary)
            .collect::<Vec<_>>();
        if primaries.len() != 1 {
            return Err(format!("素材场次 {group_id} 必须且只能有一个主机位。"));
        }
        let primary = primaries[0];
        if !primary.include_in_final {
            return Err(format!("素材场次 {group_id} 的主机位必须默认入片。"));
        }
        if group
            .iter()
            .any(|clip| clip.take_role == RenderTakeRole::Alternate && clip.include_in_final)
        {
            return Err(format!(
                "素材场次 {group_id} 的备用机位不能直接增加成片时长。"
            ));
        }
        for take in group {
            if take.candidate_id != primary.candidate_id
                || (take.source_start_seconds - primary.source_start_seconds).abs() > 0.001
                || (take.source_end_seconds - primary.source_end_seconds).abs() > 0.001
                || take.duration_mode != primary.duration_mode
                || (take.story_pre_roll_seconds - primary.story_pre_roll_seconds).abs() > 0.001
                || option_seconds_differ(
                    take.short_source_start_seconds,
                    primary.short_source_start_seconds,
                )
                || option_seconds_differ(
                    take.short_source_end_seconds,
                    primary.short_source_end_seconds,
                )
            {
                return Err(format!(
                    "素材场次 {group_id} 的所有机位必须对应同一事件和完全相同的时间段。"
                ));
            }
        }
    }
    Ok(())
}

fn option_seconds_differ(left: Option<f32>, right: Option<f32>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => (left - right).abs() > 0.001,
        (None, None) => false,
        _ => true,
    }
}

fn is_dual_edit_plan_schema(schema_version: &str) -> bool {
    schema_version == EDIT_PLAN_SCHEMA_VERSION
        || DUAL_EDIT_PLAN_SCHEMA_VERSIONS.contains(&schema_version)
}

#[tauri::command]
fn get_edit_plan(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<Option<LoadedEditPlan>, String> {
    if !valid_job_id(&job_id) {
        return Err("任务编号无效，请重新打开分析结果。".to_string());
    }
    let path = state
        .jobs_root
        .join(&job_id)
        .join("director")
        .join("edit-plan.json");
    if !path.is_file() {
        return Ok(None);
    }
    load_highlight_plan_at(&path, &state.jobs_root.join(&job_id))
}

fn load_highlight_plan_at(path: &Path, job_dir: &Path) -> Result<Option<LoadedEditPlan>, String> {
    let value: serde_json::Value = read_json(path)?;
    if is_dual_edit_plan_schema(value["schema_version"].as_str().unwrap_or_default())
        && value["plans"]
            .as_array()
            .is_some_and(|slots| !slots.iter().any(|slot| slot["mode"] == "default"))
    {
        return Ok(None);
    }
    let document = decode_highlight_document(value)?;
    validate_imported_document(&document, job_dir)?;
    load_edit_plan(document)
        .map(Some)
        .ok_or_else(|| "已保存草稿的版本或结构不受支持，原文件已保留，请勿覆盖。".to_string())
}

fn decode_highlight_document(mut value: serde_json::Value) -> Result<EditPlanDocument, String> {
    if is_dual_edit_plan_schema(value["schema_version"].as_str().unwrap_or_default()) {
        let slots = value["plans"]
            .as_array()
            .ok_or("已保存草稿结构无效，原文件已保留。")?;
        // Old creative drafts are retained on disk without loading their editing/runtime code.
        value["plans"] = serde_json::Value::Array(
            slots
                .iter()
                .filter(|slot| slot["mode"] == "default")
                .cloned()
                .collect(),
        );
        value["active_mode"] = serde_json::json!("default");
        value["clips"] = serde_json::json!([]);
    }
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn validate_imported_document(document: &EditPlanDocument, job_dir: &Path) -> Result<(), String> {
    let imported: Vec<_> = document
        .plans
        .iter()
        .filter(|p| p.mode == EditPlanMode::Default)
        .flat_map(|p| &p.clips)
        .chain(
            document
                .clips
                .iter()
                .filter(|_| !is_dual_edit_plan_schema(&document.schema_version)),
        )
        .filter(|c| c.imported.is_some())
        .collect();
    if imported.is_empty() {
        return Ok(());
    }
    let timeline: TimelineDocument = read_json(&job_dir.join("timeline/combat-events.json"))?;
    if document.source_sha256 != timeline.source_sha256 {
        return Err("导入草稿与本局录像不一致，原文件已保留。".into());
    }
    for clip in imported {
        timestamp_import::validate_anchor(
            clip.imported.as_ref().unwrap(),
            &clip.candidate_id,
            &timeline,
        )?;
    }
    Ok(())
}

/// Local file handoff for Codex; uses the same read-only analysis, validation and atomic draft writer as the UI.
pub fn import_timestamp_file(
    replay: &Path,
    plan_path: &Path,
    jobs_root: &Path,
) -> Result<serde_json::Value, String> {
    let summary = analyze_highlights_with_progress(replay, jobs_root, |_| {})
        .map_err(|error| error.to_string())?;
    let dir = jobs_root.join(&summary.job_id);
    let manifest: JobManifest = read_json(&dir.join("manifest.json"))?;
    let timeline: TimelineDocument = read_json(&dir.join("timeline/combat-events.json"))?;
    let text = timestamp_import::read_text(plan_path)?;
    let preview = timestamp_import::build_preview(
        &text,
        &plan_path.display().to_string(),
        &manifest.source,
        &timeline,
        None,
        None,
    )?;
    let mut receipt = append_timestamp_preview(jobs_root, &summary.job_id, preview)?;
    receipt["analysisWarnings"] =
        serde_json::to_value(summary.analysis_warnings).map_err(|error| error.to_string())?;
    Ok(receipt)
}

fn append_timestamp_preview(
    jobs_root: &Path,
    job_id: &str,
    preview: timestamp_import::Preview,
) -> Result<serde_json::Value, String> {
    if !valid_job_id(job_id) {
        return Err("任务编号无效。".into());
    }
    let extended = preview.schema_version == "d2h.timestamp-plan/1.2";
    let path = jobs_root.join(job_id).join("director/edit-plan.json");
    let previous = if path.is_file() {
        load_highlight_plan_at(&path, &jobs_root.join(job_id))?
    } else {
        None
    };
    let mut input: EditPlanSlotInput =
        match previous.and_then(|loaded| loaded.plans.into_iter().next()) {
            Some(slot) => serde_json::from_value(
                serde_json::to_value(slot).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?,
            None => EditPlanSlotInput {
                mode: EditPlanMode::Default,
                selected_hero: Some(preview.hero),
                highlight_rule_ids: Vec::new(),
                selected_clip_id: None,
                source_story_id: None,
                clips: Vec::new(),
                settings: RenderSettings::default(),
            },
        };
    let mut seen: HashSet<_> = input
        .clips
        .iter()
        .filter_map(|clip| clip.imported.as_ref().map(|meta| meta.source_id.clone()))
        .collect();
    let mut added = Vec::new();
    let mut corrected = Vec::new();
    let mut updated = Vec::new();
    let order: Vec<_> = preview
        .clips
        .iter()
        .map(|clip| clip.imported.source_id.clone())
        .collect();
    for (index, clip) in preview.clips.into_iter().enumerate() {
        if !seen.insert(clip.imported.source_id.clone()) {
            if let Some(existing) = input.clips.iter_mut().find(|existing| {
                existing
                    .imported
                    .as_ref()
                    .is_some_and(|meta| meta.source_id == clip.imported.source_id)
            }) {
                if existing.view_hero.as_deref() != Some(clip.hero.as_str()) {
                    corrected.push(existing.clip_id.clone());
                    existing.view_hero = Some(clip.hero.clone());
                }
                if extended {
                    existing.candidate_id = clip.id;
                    existing.imported = Some(clip.imported);
                    existing.view_hero = Some(clip.hero);
                    existing.camera_mode = clip.camera_mode;
                    existing.take_group_id = clip.take_group_id;
                    existing.take_role = clip.take_role;
                    existing.include_in_final = clip.include_in_final;
                    existing.duration_mode = ClipDurationMode::Short;
                    existing.story_pre_roll_seconds = 60.0;
                    existing.short_source_start_seconds = clip.start_seconds;
                    existing.short_source_end_seconds = clip.end_seconds;
                    existing.source_start_seconds = clip.start_seconds;
                    existing.source_end_seconds = clip.end_seconds;
                    updated.push(existing.clip_id.clone());
                }
            }
            continue;
        }
        let clip_id = format!("clip-time-{}", clip.imported.source_id);
        added.push(clip_id.clone());
        let insertion = order[index + 1..].iter().find_map(|source_id| {
            input.clips.iter().position(|existing| {
                existing
                    .imported
                    .as_ref()
                    .is_some_and(|meta| &meta.source_id == source_id)
            })
        });
        input.clips.insert(
            insertion.unwrap_or(input.clips.len()),
            EditPlanClipInput {
                clip_id,
                candidate_id: clip.id,
                imported: Some(clip.imported),
                view_hero: Some(clip.hero),
                camera_mode: clip.camera_mode,
                take_group_id: clip.take_group_id,
                take_role: clip.take_role,
                include_in_final: clip.include_in_final,
                duration_mode: ClipDurationMode::Short,
                story_pre_roll_seconds: 60.0,
                short_source_start_seconds: clip.start_seconds,
                short_source_end_seconds: clip.end_seconds,
                source_start_seconds: clip.start_seconds,
                source_end_seconds: clip.end_seconds,
            },
        );
    }
    if input.selected_clip_id.is_none() {
        input.selected_clip_id = added.first().cloned();
    }
    let total_count = input.clips.len();
    let saved = save_edit_plan_to(
        jobs_root,
        SaveEditPlanRequest {
            job_id: job_id.into(),
            active_mode: EditPlanMode::Default,
            plans: vec![input],
        },
    )?;
    Ok(
        serde_json::json!({"jobId":job_id,"draftPath":path,"addedClipIds":added,
        "correctedHeroClipIds":corrected,"updatedClipIds":updated,"totalClipCount":total_count,
        "totalDurationSeconds":saved.total_duration_seconds,"module":"highlights"}),
    )
}

#[tauri::command]
fn get_startup_replay() -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--replay" {
            return args.next().filter(|p| {
                Path::new(p).is_file()
                    && Path::new(p)
                        .extension()
                        .is_some_and(|s| s.eq_ignore_ascii_case("dem"))
            });
        }
    }
    None
}

fn load_edit_plan(document: EditPlanDocument) -> Option<LoadedEditPlan> {
    if is_dual_edit_plan_schema(&document.schema_version) {
        let default_count = document
            .plans
            .iter()
            .filter(|plan| plan.mode == EditPlanMode::Default)
            .count();
        if default_count != 1 {
            return None;
        }
        return Some(LoadedEditPlan {
            active_mode: EditPlanMode::Default,
            plans: document
                .plans
                .into_iter()
                .filter(|plan| plan.mode == EditPlanMode::Default)
                .map(load_edit_plan_slot)
                .collect(),
        });
    }
    if !LEGACY_EDIT_PLAN_SCHEMA_VERSIONS.contains(&document.schema_version.as_str()) {
        return None;
    }

    let selected_clip_id = document.clips.first().map(|clip| legacy_clip_id(clip, 0));
    Some(LoadedEditPlan {
        active_mode: EditPlanMode::Default,
        plans: vec![load_edit_plan_slot(EditPlanSlot {
            mode: EditPlanMode::Default,
            selected_hero: None,
            highlight_rule_ids: Vec::new(),
            selected_clip_id,
            source_story_id: None,
            clips: document.clips,
            settings: document.settings,
        })],
    })
}

fn legacy_clip_id(clip: &EditPlanClip, index: usize) -> String {
    if clip.clip_id.is_empty() {
        format!("clip-saved-{:02}", index + 1)
    } else {
        clip.clip_id.clone()
    }
}

fn load_edit_plan_slot(plan: EditPlanSlot) -> LoadedEditPlanSlot {
    LoadedEditPlanSlot {
        mode: plan.mode,
        selected_hero: plan.selected_hero,
        highlight_rule_ids: plan.highlight_rule_ids,
        selected_clip_id: plan.selected_clip_id,
        source_story_id: plan.source_story_id,
        clips: plan
            .clips
            .into_iter()
            .enumerate()
            .map(|(index, clip)| LoadedEditPlanClip {
                clip_id: legacy_clip_id(&clip, index),
                candidate_id: clip.candidate_id,
                imported: clip.imported,
                view_hero: clip.view_hero,
                camera_mode: normalize_user_camera_mode(clip.camera_mode),
                take_group_id: clip.take_group_id,
                take_role: clip.take_role,
                include_in_final: clip.include_in_final,
                duration_mode: clip.duration_mode,
                story_pre_roll_seconds: clip.story_pre_roll_seconds,
                short_source_start_seconds: clip
                    .short_source_start_seconds
                    .unwrap_or(clip.source_start_seconds),
                short_source_end_seconds: clip
                    .short_source_end_seconds
                    .unwrap_or(clip.source_end_seconds),
                source_start_seconds: clip.source_start_seconds,
                source_end_seconds: clip.source_end_seconds,
            })
            .collect(),
        settings: plan.settings,
    }
}

#[tauri::command]
fn get_latest_render(
    job_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<RenderResult>, String> {
    if !valid_job_id(&job_id) {
        return Err("任务编号无效，请重新打开分析结果。".to_string());
    }
    let job_dir = state.jobs_root.join(&job_id);
    let output_dir = job_dir.join("output");
    if let Some(result) = video_output::latest_highlight(&job_dir) {
        app.asset_protocol_scope()
            .allow_file(&result.output_path)
            .map_err(|e| e.to_string())?;
        return Ok(Some(result));
    }
    if !output_dir.is_dir() {
        return Ok(None);
    }

    let mut reports = fs::read_dir(&output_dir)
        .map_err(|error| error.to_string())?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
                && path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("_质量报告"))
        })
        .collect::<Vec<_>>();
    reports.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH)
    });

    let job_root = fs::canonicalize(&job_dir).map_err(|error| error.to_string())?;
    let (fallback_segment_count, fallback_source_asset_count) =
        read_json::<EditPlanDocument>(&job_dir.join("director").join("edit-plan.json"))
            .map(|plan| {
                let clips = if is_dual_edit_plan_schema(&plan.schema_version) {
                    plan.plans
                        .iter()
                        .find(|slot| slot.mode == plan.active_mode)
                        .map(|slot| slot.clips.as_slice())
                        .unwrap_or_default()
                } else {
                    plan.clips.as_slice()
                };
                (
                    clips.iter().filter(|clip| clip.include_in_final).count(),
                    clips.len(),
                )
            })
            .unwrap_or_default();

    for report_path in reports.into_iter().rev() {
        let Ok(report) = read_json::<StoredQcReport>(&report_path) else {
            continue;
        };
        if report.schema_version != RENDER_SCHEMA_VERSION {
            continue;
        }
        let configured_output = PathBuf::from(&report.output_path);
        let fallback_output = report_path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix("_质量报告"))
            .map(|name| output_dir.join(format!("{name}.mp4")));
        let output_path = if configured_output.is_file() {
            configured_output
        } else if let Some(path) = fallback_output.filter(|path| path.is_file()) {
            path
        } else {
            continue;
        };
        let output_path = fs::canonicalize(output_path).map_err(|error| error.to_string())?;
        let report_path = fs::canonicalize(report_path).map_err(|error| error.to_string())?;
        if !output_path.starts_with(&job_root) || !report_path.starts_with(&job_root) {
            continue;
        }
        let source_assets_dir = PathBuf::from(&report.source_assets_dir);
        let source_assets_dir = if source_assets_dir.is_dir() {
            fs::canonicalize(source_assets_dir)
                .ok()
                .filter(|path| path.starts_with(&job_root))
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let mut source_assets = report.source_assets;
        source_assets.retain_mut(|asset| {
            let Ok(path) = fs::canonicalize(&asset.output_path) else {
                return false;
            };
            if !path.starts_with(&job_root) {
                return false;
            }
            asset.output_path = path.display().to_string();
            true
        });
        let source_asset_count = if source_assets.is_empty() {
            fallback_source_asset_count
        } else {
            source_assets.len()
        };

        return Ok(Some(RenderResult {
            output_path: output_path.display().to_string(),
            qc_report_path: report_path.display().to_string(),
            source_assets_dir,
            source_assets,
            duration_seconds: report.duration_seconds,
            width: report.width,
            height: report.height,
            segment_count: if report.final_segment_count == 0 {
                fallback_segment_count
            } else {
                report.final_segment_count
            },
            source_asset_count,
            warnings: report.warnings,
        }));
    }

    Ok(None)
}

#[tauri::command]
async fn get_render_resume(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<RenderResumeSummary, String> {
    if !valid_job_id(&job_id) {
        return Err("任务编号无效，请重新打开分析结果。".into());
    }
    let lease = state.activities.begin(Activity::Render)?;
    let cancellation = register_render_cancellation(&state.activities, &state.render_runtime)?;
    let worker_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let _runtime_guard = RenderRuntimeGuard(worker_state.render_runtime.clone());
        let request = build_render_request(&job_id, &worker_state)?;
        inspect_resume(&request, &cancellation).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn start_render(
    job_id: String,
    on_progress: Channel<RenderProgress>,
    resume_fingerprint: Option<String>,
    output_directory: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
    desktop_module: State<'_, DesktopModule>,
) -> Result<RenderResult, String> {
    if *desktop_module != DesktopModule::Highlights {
        return Err(
            "请先交给高光剪辑，并在高光剪辑中确认开始录制。WTF模块不会直接启动游戏。".into(),
        );
    }
    if !valid_job_id(&job_id) {
        return Err("任务编号无效，请重新打开分析结果。".into());
    }
    // The renderer's own log starts after preflight. Keep a job-local attempt
    // record as well, so validation and final delivery errors remain available.
    let attempt = diagnostics::begin_attempt(&state.jobs_root, &job_id);
    let result = start_render_inner(
        job_id,
        on_progress,
        resume_fingerprint,
        output_directory,
        app,
        state,
    )
    .await;
    if let Some(attempt) = attempt {
        attempt.finish(result.as_ref().err().map(String::as_str));
    }
    result
}

async fn start_render_inner(
    job_id: String,
    on_progress: Channel<RenderProgress>,
    resume_fingerprint: Option<String>,
    output_directory: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RenderResult, String> {
    if !valid_job_id(&job_id) {
        return Err("任务编号无效，请重新打开分析结果。".to_string());
    }
    let lease = state.activities.begin(Activity::Render)?;
    let output_directory = video_output::validate_directory(
        output_directory.or_else(|| state.video_output.current()),
    )?;
    let cancellation = register_render_cancellation(&state.activities, &state.render_runtime)?;

    let worker_state = state.inner().clone();
    let progress_channel = on_progress.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let _runtime_guard = RenderRuntimeGuard(worker_state.render_runtime.clone());
        let result = build_render_request(&job_id, &worker_state).and_then(|mut request| {
            request.resume_fingerprint = resume_fingerprint;
            render(request, cancellation, |progress| {
                let _ = progress_channel.send(progress);
            })
            .map_err(|error| error.to_string())
            .and_then(|result| {
                video_output::deliver_highlight(
                    &worker_state,
                    &job_id,
                    output_directory.as_deref(),
                    result,
                )
            })
        });
        if let Err(error) = &result {
            let _ = progress_channel.send(RenderProgress {
                stage: "failed".to_string(),
                status: "failed".to_string(),
                message: error.to_string(),
                percent: 0,
                current_clip: 0,
                total_clips: 0,
            });
        }
        result.map_err(|error| error.to_string())
    })
    .await;
    joined
        .map_err(|error| error.to_string())?
        .and_then(|result| {
            app.asset_protocol_scope()
                .allow_file(&result.output_path)
                .map_err(|e| e.to_string())?;
            Ok(result)
        })
}

#[tauri::command]
fn cancel_render(state: State<'_, AppState>) -> Result<bool, String> {
    let runtime = state
        .render_runtime
        .lock()
        .map_err(|_| "成片任务状态不可用，请重启应用。".to_string())?;
    if let Some(cancellation) = &runtime.cancellation {
        cancellation.cancel();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
fn open_local_path(
    path: String,
    state: State<'_, AppState>,
    exported: State<'_, diagnostics::ExportedPackages>,
) -> Result<(), String> {
    let target = fs::canonicalize(PathBuf::from(path))
        .map_err(|_| "输出文件不存在，可能已被移动。".to_string())?;
    let jobs_root = fs::canonicalize(&state.jobs_root).map_err(|error| error.to_string())?;
    if !target.starts_with(&jobs_root)
        && !video_output::recorded_highlight_path(&jobs_root, &target)
        && !exported.allows_directory(&target)
    {
        return Err("只能打开猫猫任务目录中的输出文件。".to_string());
    }
    open_output_path(&target)
}

fn open_output_path(target: &Path) -> Result<(), String> {
    let mut command = if target.is_dir() {
        let mut command = Command::new("explorer.exe");
        command.arg(target);
        command
    } else {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler").arg(target);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开输出：{error}"))
}

#[tauri::command]
async fn close_app_after_cleanup(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    begin_render_close(&state.activities, &state.render_runtime)?;
    let activities = state.activities.clone();
    tauri::async_runtime::spawn_blocking(move || {
        activities.wait_for_close(Duration::from_secs(120))
    })
    .await
    .map_err(|error| format!("等待任务收尾失败：{error}"))??;
    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn check_for_app_update(
    app: AppHandle,
    pending_update: State<'_, PendingUpdate>,
) -> Result<Option<AppUpdateMetadata>, String> {
    {
        let runtime = pending_update
            .0
            .lock()
            .map_err(|_| "更新状态不可用，请重启应用。".to_string())?;
        if runtime.installing {
            return Err("更新正在安装，请稍候。".to_string());
        }
    }

    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法初始化更新检查：{error}"))?;
    let update = updater
        .check()
        .await
        .map_err(|error| format!("无法连接官方更新地址：{error}"))?;
    let metadata = update.as_ref().map(|update| AppUpdateMetadata {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: sanitize_update_notes(update.body.as_deref()),
        published_at: update.date.map(|date| date.to_string()),
    });

    let mut runtime = pending_update
        .0
        .lock()
        .map_err(|_| "更新状态不可用，请重启应用。".to_string())?;
    if runtime.installing {
        return Err("更新正在安装，请稍候。".to_string());
    }
    runtime.pending = update;
    Ok(metadata)
}

#[tauri::command]
async fn install_app_update(
    app: AppHandle,
    pending_update: State<'_, PendingUpdate>,
    on_event: Channel<AppUpdateEvent>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _lease = state.activities.begin(Activity::Update)?;
    let update = {
        let mut runtime = pending_update
            .0
            .lock()
            .map_err(|_| "更新状态不可用，请重启应用。".to_string())?;
        if runtime.installing {
            return Err("更新正在安装，请不要重复操作。".to_string());
        }
        let update = runtime
            .pending
            .take()
            .ok_or_else(|| "没有可安装的更新，请先检查更新。".to_string())?;
        runtime.installing = true;
        update
    };

    let mut started = false;
    let download_result = update
        .download(
            |chunk_length, content_length| {
                if !started {
                    started = true;
                    let _ = on_event.send(AppUpdateEvent::Started { content_length });
                }
                let _ = on_event.send(AppUpdateEvent::Progress { chunk_length });
            },
            || {
                let _ = on_event.send(AppUpdateEvent::Finished);
            },
        )
        .await;

    let install_result = match download_result {
        Ok(bytes) => update
            .install(bytes)
            .map_err(|error| format!("安装更新失败：{error}")),
        Err(error) => Err(format!("更新包下载或签名验证失败：{error}")),
    };
    if let Err(error) = install_result {
        state.activities.set_update_exit_ready(false)?;
        let mut runtime = pending_update
            .0
            .lock()
            .map_err(|_| "更新失败且状态无法恢复，请重启应用。")?;
        runtime.installing = false;
        runtime.pending = Some(update);
        return Err(error);
    }

    state.activities.set_update_exit_ready(true)?;
    app.restart()
}

fn sanitize_update_notes(notes: Option<&str>) -> String {
    const MAX_CHARS: usize = 2_000;
    let notes = notes.unwrap_or_default().trim();
    if notes.chars().count() <= MAX_CHARS {
        return notes.to_string();
    }
    let mut truncated = notes.chars().take(MAX_CHARS).collect::<String>();
    truncated.push('…');
    truncated
}

fn build_render_request(job_id: &str, state: &AppState) -> Result<RenderRequest, String> {
    let job_dir = state.jobs_root.join(job_id);
    let manifest = read_json::<JobManifest>(&job_dir.join("manifest.json"))
        .map_err(|_| "任务数据不存在，请重新分析录像。".to_string())?;
    let highlights =
        read_json::<HighlightDocument>(&job_dir.join("timeline").join("highlights.json"))
            .map_err(|_| "高光候选数据不完整，请重新分析录像。".to_string())?;
    let timeline =
        read_json::<TimelineDocument>(&job_dir.join("timeline").join("combat-events.json"))
            .map_err(|_| "回放时间轴不完整，请重新分析录像。".to_string())?;
    let edit_plan = decode_highlight_document(
        read_json(&job_dir.join("director/edit-plan.json"))
            .map_err(|_| "请先保存剪辑方案，再开始生成成片。".to_string())?,
    )?;
    build_render_from_documents(job_id, state, manifest, timeline, highlights, edit_plan)
}

fn build_render_from_documents(
    job_id: &str,
    state: &AppState,
    manifest: JobManifest,
    timeline: TimelineDocument,
    highlights: HighlightDocument,
    edit_plan: EditPlanDocument,
) -> Result<RenderRequest, String> {
    let job_dir = state.jobs_root.join(job_id);
    if edit_plan.source_sha256 != manifest.source.sha256
        || timeline.source_sha256 != manifest.source.sha256
        || highlights.source_sha256 != manifest.source.sha256
    {
        return Err("任务数据版本不一致，请重新分析录像。".to_string());
    }

    let (edit_clips, edit_settings) = if is_dual_edit_plan_schema(&edit_plan.schema_version) {
        let active_plan = edit_plan
            .plans
            .iter()
            .find(|plan| plan.mode == EditPlanMode::Default)
            .ok_or_else(|| "当前剪辑方案不存在，请重新保存。".to_string())?;
        if active_plan.clips.is_empty() {
            return Err("当前剪辑方案没有可导出的片段。".to_string());
        }
        (&active_plan.clips, &active_plan.settings)
    } else if LEGACY_EDIT_PLAN_SCHEMA_VERSIONS.contains(&edit_plan.schema_version.as_str()) {
        (&edit_plan.clips, &edit_plan.settings)
    } else {
        return Err("剪辑方案版本不受支持，请重新保存。".to_string());
    };

    let mut clips = Vec::with_capacity(edit_clips.len());
    for clip in edit_clips {
        let candidate = highlights
            .candidates
            .iter()
            .find(|candidate| candidate.id == clip.candidate_id);
        let (anchor_seconds, anchor_tick) = if let Some(imported) = &clip.imported {
            timestamp_import::validate_anchor(imported, &clip.candidate_id, &timeline)?;
            (imported.anchor_seconds, imported.anchor_tick)
        } else {
            let candidate =
                candidate.ok_or_else(|| format!("找不到候选片段 {}。", clip.candidate_id))?;
            (candidate.peak_seconds, candidate.anchor_tick)
        };
        let inset = ((clip.source_end_seconds - clip.source_start_seconds) * 0.1).min(0.25);
        let style_peak = anchor_seconds.clamp(
            clip.source_start_seconds + inset,
            clip.source_end_seconds - inset,
        );
        clips.push(RenderClip {
            clip_id: clip.clip_id.clone(),
            candidate_id: clip.candidate_id.clone(),
            view_hero: clip
                .view_hero
                .clone()
                .or_else(|| candidate.and_then(|c| c.primary_hero.clone())),
            camera_mode: normalize_user_camera_mode(clip.camera_mode.clone()),
            take_group_id: clip.take_group_id.clone(),
            take_role: clip.take_role,
            include_in_final: clip.include_in_final,
            source_start_seconds: clip.source_start_seconds,
            source_peak_seconds: style_peak,
            source_end_seconds: clip.source_end_seconds,
            anchor_seconds,
            anchor_tick,
        });
    }

    Ok(RenderRequest {
        job_id: job_id.to_string(),
        source_sha256: manifest.source.sha256,
        job_dir,
        source_replay: PathBuf::from(manifest.source.path),
        dota2_exe: state
            .dota2_runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .executable
            .clone()
            .ok_or_else(|| "未找到 Dota 2，当前只能分析录像。".to_string())?,
        ffmpeg_exe: state
            .ffmpeg_exe
            .clone()
            .ok_or_else(|| "未找到 FFmpeg，请使用完整版目录或重新安装。".to_string())?,
        ffprobe_exe: state
            .ffprobe_exe
            .clone()
            .ok_or_else(|| "未找到 FFprobe，请使用完整版目录或重新安装。".to_string())?,
        timeline,
        clips,
        settings: manual_render_settings(edit_settings.clone()),
        resume_fingerprint: None,
    })
}

fn manual_render_settings(mut settings: RenderSettings) -> RenderSettings {
    settings.camera_style = CameraStyle::AutoDirector;
    settings.slow_motion = false;
    settings.replay_emphasis = false;
    settings.bgm_mode = BgmMode::GameOnly;
    settings.custom_bgm_path = None;
    settings.game_audio_volume = 1.0;
    settings.bgm_volume = 0.0;
    settings.impact_sfx = false;
    settings.system_narration = false;
    settings
}

fn normalize_user_camera_mode(mode: ClipCameraMode) -> ClipCameraMode {
    match mode {
        ClipCameraMode::HeroChase => ClipCameraMode::HeroChase,
        ClipCameraMode::Directed
        | ClipCameraMode::FreeCamera
        | ClipCameraMode::PlayerPerspective => ClipCameraMode::PlayerPerspective,
    }
}

struct RenderRuntimeGuard(Arc<Mutex<RenderRuntime>>);

fn register_render_cancellation(
    activities: &Activities,
    runtime: &Mutex<RenderRuntime>,
) -> Result<CancellationToken, String> {
    let cancellation = {
        let mut runtime = runtime
            .lock()
            .map_err(|_| "成片任务状态不可用，请重启应用。".to_string())?;
        if runtime.active {
            return Err("已有成片任务正在运行，请等待完成或先取消。".to_string());
        }
        let cancellation = CancellationToken::new();
        runtime.active = true;
        runtime.cancellation = Some(cancellation.clone());
        cancellation
    };
    // Close can start after acquiring the activity lease but before publishing
    // this token. Check after publication; the close path checks the token after
    // marking closing, so either interleaving observes and cancels the render.
    // Do not hold the runtime lock while reading the activity state.
    if activities.is_closing() {
        cancellation.cancel();
    }
    Ok(cancellation)
}

fn begin_render_close(
    activities: &Activities,
    runtime: &Mutex<RenderRuntime>,
) -> Result<(), String> {
    activities.begin_close()?;
    let runtime = runtime.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(cancellation) = &runtime.cancellation {
        cancellation.cancel();
    }
    Ok(())
}

impl Drop for RenderRuntimeGuard {
    fn drop(&mut self) {
        reset_render_runtime(&self.0);
    }
}

fn valid_frame_duration(start: f32, end: f32) -> bool {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
        return false;
    }
    let frames = (f64::from(end) * 30.0).round() - (f64::from(start) * 30.0).round();
    (30.0..=3000.0).contains(&frames)
}

fn reset_render_runtime(runtime: &Arc<Mutex<RenderRuntime>>) {
    if let Ok(mut runtime) = runtime.lock() {
        runtime.active = false;
        runtime.cancellation = None;
    }
}

fn valid_job_id(value: &str) -> bool {
    value.starts_with("d2h-")
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_clip_id(value: &str) -> bool {
    value.starts_with("clip-")
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_replay_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn mark_job_recent(jobs_root: &Path, job_id: &str) -> Result<(), String> {
    if !valid_job_id(job_id) {
        return Err("任务编号无效，无法更新最近任务。".to_string());
    }
    let job_dir = jobs_root.join(job_id);
    if !job_dir.is_dir() {
        return Err("任务目录不存在，无法更新最近任务。".to_string());
    }
    let last_opened_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "系统时间无效，无法更新最近任务。".to_string())?
        .as_secs();
    let mut bytes = serde_json::to_vec_pretty(&RecentJobActivity {
        last_opened_unix_seconds,
    })
    .map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    fs::write(job_dir.join("recent-activity.json"), bytes).map_err(|error| error.to_string())
}

fn source_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn validate_dota2_executable(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("请输入 dota2.exe 的完整路径，或使用浏览按钮选择。".to_string());
    }
    let valid_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("dota2.exe"));
    if !valid_name {
        return Err("请选择 Dota 2 的 dota2.exe 文件。".to_string());
    }
    if !path.is_file() {
        return Err("这个 dota2.exe 不存在，请检查路径或重新选择。".to_string());
    }
    Ok(path.to_path_buf())
}

fn persist_dota2_path(settings_path: &Path, executable: Option<&Path>) -> Result<(), String> {
    if let Some(executable) = executable {
        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let settings = Dota2PathSettings {
            schema_version: DOTA2_SETTINGS_SCHEMA_VERSION.to_string(),
            executable_path: executable.display().to_string(),
        };
        let mut bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        fs::write(settings_path, bytes).map_err(|error| error.to_string())
    } else {
        match fs::remove_file(settings_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

fn configured_dota2_runtime(settings_path: &Path) -> Option<Dota2Runtime> {
    let settings = read_json::<Dota2PathSettings>(settings_path).ok()?;
    if settings.schema_version != DOTA2_SETTINGS_SCHEMA_VERSION {
        return None;
    }
    let executable = validate_dota2_executable(Path::new(&settings.executable_path)).ok()?;
    Some(Dota2Runtime {
        executable: Some(executable),
        source: Dota2PathSource::Custom,
    })
}

fn automatic_dota2_runtime() -> Dota2Runtime {
    match find_dota2() {
        Some(executable) => Dota2Runtime {
            executable: Some(executable),
            source: Dota2PathSource::Automatic,
        },
        None => Dota2Runtime {
            executable: None,
            source: Dota2PathSource::Missing,
        },
    }
}

fn resolve_dota2_runtime(settings_path: &Path) -> Dota2Runtime {
    configured_dota2_runtime(settings_path).unwrap_or_else(automatic_dota2_runtime)
}

fn find_dota2() -> Option<PathBuf> {
    if let Some(path) = env::var_os("DOTA2_EXE").map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }

    for letter in b'C'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", letter as char));
        if !root.is_dir() {
            continue;
        }
        for steam_root in [
            root.join("Program Files (x86)").join("Steam"),
            root.join("SteamLibrary"),
            root.join("Steam"),
        ] {
            let candidate = steam_root
                .join("steamapps")
                .join("common")
                .join("dota 2 beta")
                .join("game")
                .join("bin")
                .join("win64")
                .join("dota2.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn replay_directory_from_dota2(dota2_exe: &Path) -> Option<PathBuf> {
    let game_directory = dota2_exe.ancestors().find(|directory| {
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("game"))
    })?;
    let dota_directory = game_directory.join("dota");
    dota_directory
        .is_dir()
        .then(|| dota_directory.join("replays"))
}

fn find_media_tool(
    file_name: &str,
    environment_name: &str,
    executable_dir: &Path,
    resource_dir: &Path,
    root: &Path,
) -> Option<PathBuf> {
    if let Some(path) = env::var_os(environment_name).map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    for candidate in [
        executable_dir
            .join("tools")
            .join("ffmpeg")
            .join("bin")
            .join(file_name),
        resource_dir
            .join("tools")
            .join("ffmpeg")
            .join("bin")
            .join(file_name),
        root.join("tools")
            .join("ffmpeg")
            .join("bin")
            .join(file_name),
    ] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|directory| directory.join(file_name))
            .find(|candidate| candidate.is_file())
    })
}

fn project_root() -> PathBuf {
    if let Some(path) = env::var_os("D2H_PROJECT_ROOT").map(PathBuf::from) {
        return path;
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("desktop crate must remain under apps/<name>/src-tauri")
        .to_path_buf()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_with_module(DesktopModule::Highlights);
}

/// Independent entry point: its own process and jobs, sharing only infrastructure.
pub fn run_wtf() {
    run_with_module(DesktopModule::Wtf);
}

fn run_with_module(module: DesktopModule) {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(PendingUpdate::default())
        .manage(diagnostics::ExportedPackages::default())
        .manage(module)
        .setup(move |app| {
            let root = project_root();
            let executable_dir = env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| root.clone());
            let resource_dir = app.path().resource_dir()?;
            let app_local_data_dir = app.path().app_local_data_dir()?;
            let jobs_root = if module == DesktopModule::Wtf {
                app_local_data_dir.join("wtf").join("jobs")
            } else if let Some(configured_root) = env::var_os("D2H_PROJECT_ROOT").map(PathBuf::from)
            {
                configured_root.join("jobs")
            } else if executable_dir.join("Cargo.toml").is_file()
                || executable_dir.join("jobs").is_dir()
            {
                executable_dir.join("jobs")
            } else if cfg!(debug_assertions) {
                root.join("jobs")
            } else {
                app_local_data_dir.join("jobs")
            };
            fs::create_dir_all(&jobs_root)?;
            fs::create_dir_all(&app_local_data_dir)?;
            if module == DesktopModule::Wtf
                && let Some(window) = app.get_webview_window("main")
            {
                window.set_title("WTF 剪辑")?;
            }
            app.asset_protocol_scope()
                .allow_directory(&jobs_root, true)?;
            let ffmpeg_exe = find_media_tool(
                "ffmpeg.exe",
                "FFMPEG_EXE",
                &executable_dir,
                &resource_dir,
                &root,
            );
            let ffprobe_exe = find_media_tool(
                "ffprobe.exe",
                "FFPROBE_EXE",
                &executable_dir,
                &resource_dir,
                &root,
            );
            let dota2_settings_path = app_local_data_dir.join(DOTA2_SETTINGS_FILE);
            app.manage(AppState {
                jobs_root,
                dota2_runtime: Arc::new(Mutex::new(resolve_dota2_runtime(&dota2_settings_path))),
                dota2_settings_path,
                video_output: video_output::Settings::load(
                    app_local_data_dir.join(video_output::FILE),
                ),
                ffmpeg_exe,
                ffprobe_exe,
                render_runtime: Arc::new(Mutex::new(RenderRuntime::default())),
                activities: Activities::default(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_desktop_module,
            wtf::analyze_wtf_dem,
            wtf::get_wtf_draft,
            wtf::save_wtf_draft,
            wtf::export_wtf_plan,
            wtf::handoff_wtf_plan,
            get_capabilities,
            get_startup_replay,
            set_dota2_executable,
            video_output::set_video_output_directory,
            resolve_replay_by_id,
            replay_library::list_replays,
            external_modules::open_external_tool,
            get_recent_jobs,
            analyze_dem,
            save_edit_plan,
            preview_timestamp_plan,
            get_edit_plan,
            get_latest_render,
            get_render_resume,
            diagnostics::export_render_diagnostics,
            cache_cleanup::inspect_render_cache,
            cache_cleanup::clean_render_cache,
            start_render,
            cancel_render,
            open_local_path,
            close_app_after_cleanup,
            check_for_app_update,
            install_app_update
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && let Some(state) = window.try_state::<AppState>()
                && !state.activities.exit_ready()
            {
                // The frontend close listener must persist both drafts before requesting cleanup.
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("unable to start Cat Cut Assistant")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event
                && let Some(state) = app.try_state::<AppState>()
                && !state.activities.exit_ready()
            {
                api.prevent_exit();
                let _ = app.emit("app-close-requested", ());
            }
        });
}

#[cfg(test)]
mod highlight_plan_tests;
#[cfg(test)]
mod resume_tests;

#[cfg(test)]
mod tests {
    use super::{
        ClipDurationMode, DOTA2_SETTINGS_FILE, DOTA2_SETTINGS_SCHEMA_VERSION, Dota2PathSettings,
        Dota2PathSource, EDIT_PLAN_SCHEMA_VERSION, EditPlanClip, EditPlanDocument, EditPlanMode,
        EditPlanSlot, archive_edit_plan_for_reanalysis, configured_dota2_runtime, load_edit_plan,
        manual_render_settings, mark_job_recent, normalize_user_camera_mode, persist_dota2_path,
        replay_directory_from_dota2, resolve_replay_by_id, sanitize_update_notes, valid_clip_id,
        valid_job_id, valid_replay_id, validate_dota2_executable, validate_edit_plan_take_groups,
    };
    use d2_highlights_renderer::{
        BgmMode, CameraStyle, ClipCameraMode, RenderSettings, RenderTakeRole,
    };
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn close_before_render_token_publication_cancels_new_token() {
        let activities = super::Activities::default();
        let runtime = std::sync::Mutex::new(super::RenderRuntime::default());
        let lease = activities.begin(super::Activity::Render).unwrap();
        super::begin_render_close(&activities, &runtime).unwrap();
        assert!(runtime.lock().unwrap().cancellation.is_none());
        let cancellation = super::register_render_cancellation(&activities, &runtime).unwrap();
        assert!(cancellation.is_cancelled());
        drop(lease);
        activities
            .wait_for_close(std::time::Duration::ZERO)
            .unwrap();
        assert!(activities.exit_ready());
    }

    #[test]
    fn close_after_render_token_publication_cancels_existing_token() {
        let activities = super::Activities::default();
        let runtime = std::sync::Mutex::new(super::RenderRuntime::default());
        let lease = activities.begin(super::Activity::Render).unwrap();
        let cancellation = super::register_render_cancellation(&activities, &runtime).unwrap();
        assert!(!cancellation.is_cancelled());
        super::begin_render_close(&activities, &runtime).unwrap();
        assert!(cancellation.is_cancelled());
        drop(lease);
        activities
            .wait_for_close(std::time::Duration::ZERO)
            .unwrap();
        assert!(activities.exit_ready());
    }

    #[test]
    fn duration_limits_use_frames_despite_f32_rounding() {
        assert!(super::valid_frame_duration(28.3, 128.3));
        assert!(super::valid_frame_duration(1900.3, 2000.3));
        assert!(super::valid_frame_duration(28.3, 29.3));
        assert!(!super::valid_frame_duration(28.3, 128.34));
        assert!(!super::valid_frame_duration(28.3, 29.26));
        assert!(!super::valid_frame_duration(f32::NAN, 100.0));
        assert!(!super::valid_frame_duration(-1.0, 1.0));
    }

    #[test]
    fn old_dual_drafts_load_only_the_highlight_workspace() {
        let value = serde_json::json!({
            "schema_version": EDIT_PLAN_SCHEMA_VERSION,
            "source_sha256": "source",
            "job_id": "d2h-0000000000000000",
            "active_mode": "default",
            "plans": [
                {"mode":"default", "clips":[], "settings":RenderSettings::default()},
                {"mode":"wtf", "clips":[], "settings":RenderSettings::default()}
            ],
            "mode": "dual", "updated_unix_seconds":0,
            "clips":[], "settings":RenderSettings::default()
        });
        let document: EditPlanDocument = serde_json::from_value(value).unwrap();
        let loaded = load_edit_plan(document).unwrap();
        assert_eq!(loaded.plans.len(), 1);
        assert!(loaded.plans.iter().all(|plan| plan.clips.is_empty()));
    }

    #[test]
    fn edit_plan_job_id_stays_inside_jobs_root() {
        assert!(valid_job_id("d2h-ff5145119d7415b3"));
        assert!(!valid_job_id("../d2h-ff5145119d7415b3"));
        assert!(!valid_job_id("d2h-.."));
        assert!(!valid_job_id("other-job"));
    }

    #[test]
    fn recent_job_activity_is_persisted_inside_the_job() {
        let root = tempdir().expect("create recent jobs root");
        let job_id = "d2h-ff5145119d7415b3";
        fs::create_dir_all(root.path().join(job_id)).expect("create job directory");

        mark_job_recent(root.path(), job_id).expect("mark job recent");
        let activity: super::RecentJobActivity =
            super::read_json(&root.path().join(job_id).join("recent-activity.json"))
                .expect("read recent activity");

        assert!(activity.last_opened_unix_seconds > 0);
        assert!(mark_job_recent(root.path(), "../outside").is_err());
    }

    #[test]
    fn reanalysis_archives_the_existing_edit_plan_without_deleting_it() {
        let root = tempdir().expect("create jobs root");
        let job_id = "d2h-ff5145119d7415b3";
        let director = root.path().join(job_id).join("director");
        fs::create_dir_all(&director).expect("create director directory");
        let saved = serde_json::to_vec(&serde_json::json!({
            "schema_version": EDIT_PLAN_SCHEMA_VERSION, "plans": [{"mode":"default"}]
        }))
        .unwrap();
        fs::write(director.join("edit-plan.json"), &saved).expect("write edit plan");

        let backup = archive_edit_plan_for_reanalysis(root.path(), job_id)
            .expect("archive edit plan")
            .expect("backup path");

        assert!(!director.join("edit-plan.json").exists());
        assert_eq!(fs::read(backup).expect("read backup"), saved);
        assert!(
            archive_edit_plan_for_reanalysis(root.path(), job_id)
                .expect("archive absent plan")
                .is_none()
        );
        assert!(archive_edit_plan_for_reanalysis(root.path(), "../outside").is_err());
    }

    #[test]
    fn clip_ids_are_stable_and_path_safe() {
        assert!(valid_clip_id("clip-recommended-01"));
        assert!(valid_clip_id("clip-123e4567-e89b-12d3-a456-426614174000"));
        assert!(!valid_clip_id("../clip-01"));
        assert!(!valid_clip_id("hl-001"));
    }

    #[test]
    fn replay_ids_are_digits_only_and_path_safe() {
        assert!(valid_replay_id("1234567890"));
        assert!(valid_replay_id("1"));
        assert!(!valid_replay_id(""));
        assert!(!valid_replay_id("1234567890.dem"));
        assert!(!valid_replay_id("../1234567890"));
        assert!(!valid_replay_id("123456789012345678901"));
    }

    #[test]
    fn replay_directory_is_derived_from_the_detected_dota_install() {
        let root = tempdir().expect("create temporary Steam library");
        let game_directory = root
            .path()
            .join("CustomSteamLibrary")
            .join("steamapps")
            .join("common")
            .join("dota 2 beta")
            .join("game");
        fs::create_dir_all(game_directory.join("dota")).expect("create Dota content directory");
        let path = game_directory.join("bin").join("win64").join("dota2.exe");
        assert_eq!(
            replay_directory_from_dota2(&path),
            Some(game_directory.join("dota").join("replays"))
        );
    }

    #[test]
    fn custom_dota2_path_is_validated_and_persisted_per_machine() {
        let local_data = tempdir().expect("create app local data");
        let executable = local_data.path().join("custom-steam").join("dota2.exe");
        fs::create_dir_all(executable.parent().expect("dota parent"))
            .expect("create Dota directory");
        fs::write(&executable, b"fixture").expect("create Dota executable fixture");
        let settings_path = local_data.path().join(DOTA2_SETTINGS_FILE);

        let validated = validate_dota2_executable(&executable).expect("validate Dota path");
        persist_dota2_path(&settings_path, Some(&validated)).expect("persist Dota path");
        let settings: Dota2PathSettings =
            super::read_json(&settings_path).expect("read Dota path settings");
        let runtime = configured_dota2_runtime(&settings_path).expect("load custom Dota path");

        assert_eq!(settings.schema_version, DOTA2_SETTINGS_SCHEMA_VERSION);
        assert_eq!(Path::new(&settings.executable_path), executable);
        assert_eq!(runtime.executable.as_deref(), Some(executable.as_path()));
        assert!(matches!(runtime.source, Dota2PathSource::Custom));
    }

    #[test]
    fn custom_dota2_path_rejects_other_files_and_can_be_reset() {
        let local_data = tempdir().expect("create app local data");
        let wrong_executable = local_data.path().join("steam.exe");
        fs::write(&wrong_executable, b"fixture").expect("create wrong executable fixture");
        assert!(validate_dota2_executable(&wrong_executable).is_err());

        let dota2_executable = local_data.path().join("dota2.exe");
        fs::write(&dota2_executable, b"fixture").expect("create Dota executable fixture");
        let settings_path = local_data.path().join(DOTA2_SETTINGS_FILE);
        persist_dota2_path(&settings_path, Some(&dota2_executable)).expect("persist Dota path");
        persist_dota2_path(&settings_path, None).expect("reset Dota path");
        assert!(!settings_path.exists());
    }

    #[test]
    fn replay_lookup_finds_a_valid_source2_dem() {
        let directory = tempdir().expect("create replay directory");
        let replay_path = directory.path().join("1234567890.dem");
        let mut replay = fs::File::create(&replay_path).expect("create replay");
        replay.write_all(b"PBDEMS2\0fixture").expect("write replay");

        let result = resolve_replay_by_id(
            directory.path().display().to_string(),
            "1234567890".to_string(),
        )
        .expect("resolve replay");

        assert_eq!(result.replay_id, "1234567890");
        assert_eq!(Path::new(&result.path), replay_path);
    }

    #[test]
    fn replay_lookup_reports_a_missing_number() {
        let directory = tempdir().expect("create replay directory");
        let error = resolve_replay_by_id(
            directory.path().display().to_string(),
            "1234567891".to_string(),
        )
        .expect_err("missing replay must fail");
        assert!(error.contains("1234567891.dem"));
    }

    #[test]
    fn manual_product_contract_disables_generated_audio_and_retiming() {
        let settings = manual_render_settings(RenderSettings {
            camera_style: CameraStyle::HeroFocus,
            slow_motion: true,
            replay_emphasis: true,
            bgm_mode: BgmMode::Original,
            custom_bgm_path: Some("music.mp3".to_string()),
            game_audio_volume: 0.5,
            bgm_volume: 0.5,
            impact_sfx: true,
            system_narration: true,
            ..RenderSettings::default()
        });

        assert_eq!(settings.camera_style, CameraStyle::AutoDirector);
        assert_eq!(settings.bgm_mode, BgmMode::GameOnly);
        assert_eq!(settings.game_audio_volume, 1.0);
        assert!(!settings.slow_motion);
        assert!(!settings.replay_emphasis);
        assert!(!settings.impact_sfx);
        assert!(!settings.system_narration);
    }

    #[test]
    fn update_notes_are_trimmed_and_bounded() {
        assert_eq!(sanitize_update_notes(Some("  修复更新  ")), "修复更新");
        let long = "猫".repeat(2_100);
        let sanitized = sanitize_update_notes(Some(&long));
        assert_eq!(sanitized.chars().count(), 2_001);
        assert!(sanitized.ends_with('…'));
    }

    #[test]
    fn user_camera_contract_defaults_legacy_modes_to_player_view() {
        assert_eq!(
            normalize_user_camera_mode(ClipCameraMode::Directed),
            ClipCameraMode::PlayerPerspective
        );
        assert_eq!(
            normalize_user_camera_mode(ClipCameraMode::FreeCamera),
            ClipCameraMode::PlayerPerspective
        );
        assert_eq!(
            normalize_user_camera_mode(ClipCameraMode::HeroChase),
            ClipCameraMode::HeroChase
        );
        assert_eq!(ClipCameraMode::default(), ClipCameraMode::PlayerPerspective);
    }

    #[test]
    fn synchronized_edit_plan_pair_is_valid() {
        let primary = paired_clip("clip-player", RenderTakeRole::Primary, true, 10.0, 20.0);
        let alternate = paired_clip("clip-close", RenderTakeRole::Alternate, false, 10.0, 20.0);

        validate_edit_plan_take_groups(&[primary, alternate]).expect("valid synchronized pair");
    }

    #[test]
    fn synchronized_edit_plan_rejects_mismatched_timecodes() {
        let primary = paired_clip("clip-player", RenderTakeRole::Primary, true, 10.0, 20.0);
        let alternate = paired_clip("clip-close", RenderTakeRole::Alternate, false, 11.0, 20.0);

        let error = validate_edit_plan_take_groups(&[primary, alternate])
            .expect_err("mismatched timecodes must fail");

        assert!(error.contains("完全相同的时间段"));
    }

    #[test]
    fn stale_edit_plan_does_not_override_new_detector_results() {
        let stale = EditPlanDocument {
            schema_version: "d2h.edit-plan/1.2".to_string(),
            source_sha256: "source".to_string(),
            job_id: "d2h-0000000000000000".to_string(),
            active_mode: EditPlanMode::Default,
            plans: Vec::new(),
            mode: "manual".to_string(),
            updated_unix_seconds: 0,
            clips: Vec::new(),
            settings: RenderSettings::default(),
        };

        assert!(load_edit_plan(stale).is_none());
    }

    #[test]
    fn legacy_edit_plan_migrates_into_default_slot() {
        let legacy = EditPlanDocument {
            schema_version: "d2h.edit-plan/1.4".to_string(),
            source_sha256: "source".to_string(),
            job_id: "d2h-0000000000000000".to_string(),
            active_mode: EditPlanMode::Default,
            plans: Vec::new(),
            mode: "manual".to_string(),
            updated_unix_seconds: 0,
            clips: vec![paired_clip(
                "clip-player",
                RenderTakeRole::Primary,
                true,
                10.0,
                20.0,
            )],
            settings: RenderSettings::default(),
        };

        let loaded = load_edit_plan(legacy).expect("legacy plan should migrate");

        assert_eq!(loaded.active_mode, EditPlanMode::Default);
        assert_eq!(loaded.plans.len(), 1);
        assert_eq!(loaded.plans[0].mode, EditPlanMode::Default);
        assert_eq!(loaded.plans[0].clips.len(), 1);
    }

    #[test]
    fn version_1_5_dual_plan_loads_with_short_duration_defaults() {
        let document: EditPlanDocument = serde_json::from_value(serde_json::json!({
            "schema_version": "d2h.edit-plan/1.5",
            "source_sha256": "source",
            "job_id": "d2h-0000000000000000",
            "active_mode": "default",
            "plans": [
                {
                    "mode": "default",
                    "clips": [{
                        "clip_id": "clip-player",
                        "candidate_id": "hk-001",
                        "view_hero": "npc_dota_hero_mirana",
                        "camera_mode": "player_perspective",
                        "take_group_id": "story-001",
                        "take_role": "primary",
                        "include_in_final": true,
                        "source_start_seconds": 10.0,
                        "source_end_seconds": 20.0
                    }]
                },
                { "mode": "wtf", "clips": [] }
            ],
            "mode": "dual",
            "updated_unix_seconds": 0
        }))
        .expect("deserialize 1.5 dual plan");

        let loaded = load_edit_plan(document).expect("1.5 dual plan should load");
        let clip = &loaded.plans[0].clips[0];

        assert_eq!(clip.duration_mode, ClipDurationMode::Short);
        assert_eq!(clip.story_pre_roll_seconds, 60.0);
        assert_eq!(clip.short_source_start_seconds, 10.0);
        assert_eq!(clip.short_source_end_seconds, 20.0);
    }

    #[test]
    fn old_dual_edit_plan_always_opens_the_highlight_slot() {
        let document = EditPlanDocument {
            schema_version: EDIT_PLAN_SCHEMA_VERSION.to_string(),
            source_sha256: "source".to_string(),
            job_id: "d2h-0000000000000000".to_string(),
            active_mode: EditPlanMode::Wtf,
            plans: vec![
                EditPlanSlot {
                    mode: EditPlanMode::Default,
                    selected_hero: Some("npc_dota_hero_mirana".to_string()),
                    highlight_rule_ids: vec!["hero_kills".to_string()],
                    selected_clip_id: Some("clip-default".to_string()),
                    source_story_id: None,
                    clips: vec![paired_clip(
                        "clip-default",
                        RenderTakeRole::Primary,
                        true,
                        10.0,
                        20.0,
                    )],
                    settings: RenderSettings::default(),
                },
                EditPlanSlot {
                    mode: EditPlanMode::Wtf,
                    selected_hero: Some("npc_dota_hero_mirana".to_string()),
                    highlight_rule_ids: Vec::new(),
                    selected_clip_id: Some("clip-wtf".to_string()),
                    source_story_id: Some("story-001".to_string()),
                    clips: vec![paired_clip(
                        "clip-wtf",
                        RenderTakeRole::Primary,
                        true,
                        30.0,
                        40.0,
                    )],
                    settings: RenderSettings::default(),
                },
            ],
            mode: "dual".to_string(),
            updated_unix_seconds: 0,
            clips: Vec::new(),
            settings: RenderSettings::default(),
        };

        let loaded = load_edit_plan(document).expect("dual plan should load");

        assert_eq!(loaded.active_mode, EditPlanMode::Default);
        assert_eq!(loaded.plans.len(), 1);
        assert_eq!(loaded.plans[0].clips[0].clip_id, "clip-default");
    }

    fn paired_clip(
        clip_id: &str,
        take_role: RenderTakeRole,
        include_in_final: bool,
        source_start_seconds: f32,
        source_end_seconds: f32,
    ) -> EditPlanClip {
        EditPlanClip {
            clip_id: clip_id.to_string(),
            candidate_id: "hk-001".to_string(),
            imported: None,
            view_hero: Some("npc_dota_hero_mirana".to_string()),
            camera_mode: if take_role == RenderTakeRole::Primary {
                ClipCameraMode::PlayerPerspective
            } else {
                ClipCameraMode::HeroChase
            },
            take_group_id: Some("story-001".to_string()),
            take_role,
            include_in_final,
            duration_mode: ClipDurationMode::Short,
            story_pre_roll_seconds: 60.0,
            short_source_start_seconds: Some(source_start_seconds),
            short_source_end_seconds: Some(source_end_seconds),
            source_start_seconds,
            source_end_seconds,
        }
    }
}
