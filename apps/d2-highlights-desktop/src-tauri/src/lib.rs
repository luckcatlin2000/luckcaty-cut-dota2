use d2_highlights_core::{DirectorDocument, HighlightDocument, JobManifest, TimelineDocument};
use d2_highlights_pipeline::{AnalysisProgress, AnalysisSummary, analyze_dem_with_progress};
use d2_highlights_renderer::{
    BgmMode, CameraStyle, CancellationToken, ClipCameraMode, RENDER_SCHEMA_VERSION, RenderClip,
    RenderProgress, RenderRequest, RenderResult, RenderSettings, render,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;
use tauri::{Manager, State};

const EDIT_PLAN_SCHEMA_VERSION: &str = "d2h.edit-plan/1.3";

#[derive(Clone)]
struct AppState {
    jobs_root: PathBuf,
    dota2_exe: Option<PathBuf>,
    ffmpeg_exe: Option<PathBuf>,
    ffprobe_exe: Option<PathBuf>,
    render_runtime: Arc<Mutex<RenderRuntime>>,
}

#[derive(Default)]
struct RenderRuntime {
    active: bool,
    cancellation: Option<CancellationToken>,
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
    mode: String,
    clips: Vec<EditPlanClipInput>,
    #[serde(default)]
    settings: RenderSettings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditPlanClipInput {
    clip_id: String,
    candidate_id: String,
    view_hero: Option<String>,
    #[serde(default)]
    camera_mode: ClipCameraMode,
    source_start_seconds: f32,
    source_end_seconds: f32,
}

#[derive(Deserialize, Serialize)]
struct EditPlanDocument {
    schema_version: String,
    source_sha256: String,
    job_id: String,
    mode: String,
    updated_unix_seconds: u64,
    clips: Vec<EditPlanClip>,
    #[serde(default)]
    settings: RenderSettings,
}

#[derive(Deserialize, Serialize)]
struct EditPlanClip {
    #[serde(default)]
    clip_id: String,
    candidate_id: String,
    view_hero: Option<String>,
    #[serde(default)]
    camera_mode: ClipCameraMode,
    source_start_seconds: f32,
    source_end_seconds: f32,
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
    mode: String,
    clips: Vec<LoadedEditPlanClip>,
    settings: RenderSettings,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadedEditPlanClip {
    clip_id: String,
    candidate_id: String,
    view_hero: Option<String>,
    camera_mode: ClipCameraMode,
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
    warnings: Vec<String>,
}

#[tauri::command]
fn get_capabilities(state: State<'_, AppState>) -> Capabilities {
    let ffmpeg_found = state.ffmpeg_exe.is_some();
    let ffprobe_found = state.ffprobe_exe.is_some();
    let dota2_found = state.dota2_exe.is_some();
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
        recommended_replay_directory: state
            .dota2_exe
            .as_deref()
            .and_then(replay_directory_from_dota2)
            .map(|path| path.display().to_string()),
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
        let director =
            read_json::<DirectorDocument>(&job_dir.join("director").join("plan.json")).ok();
        jobs.push(RecentJob {
            job_id: manifest.job_id,
            source_name: source_name(&manifest.source.path),
            source_path: manifest.source.path,
            byte_length: manifest.source.byte_length,
            candidate_count: highlights
                .as_ref()
                .map(|document| document.candidates.len())
                .unwrap_or_default(),
            duration_seconds: director
                .as_ref()
                .map(|document| document.total_duration_seconds)
                .unwrap_or_default(),
            created_unix_seconds: manifest.created_unix_seconds,
        });
    }

    jobs.sort_by_key(|job| std::cmp::Reverse(job.created_unix_seconds));
    jobs.truncate(12);
    Ok(jobs)
}

#[tauri::command]
async fn analyze_dem(
    dem_path: String,
    on_progress: Channel<AnalysisProgress>,
    state: State<'_, AppState>,
) -> Result<AnalysisSummary, String> {
    let dem_path = PathBuf::from(dem_path);
    let jobs_root = state.jobs_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        analyze_dem_with_progress(&dem_path, &jobs_root, |progress| {
            let _ = on_progress.send(progress);
        })
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn save_edit_plan(
    request: SaveEditPlanRequest,
    state: State<'_, AppState>,
) -> Result<SaveEditPlanResult, String> {
    if !valid_job_id(&request.job_id) {
        return Err("任务编号无效，请重新打开分析结果。".to_string());
    }
    if request.mode != "manual" && request.mode != "automatic" && request.mode != "review" {
        return Err("未知的剪辑模式。".to_string());
    }
    if request.clips.is_empty() {
        return Err("请至少选择一个高光片段。".to_string());
    }

    let job_dir = state.jobs_root.join(&request.job_id);
    let manifest = read_json::<JobManifest>(&job_dir.join("manifest.json"))
        .map_err(|_| "任务数据不存在，请重新分析录像。".to_string())?;
    let highlights =
        read_json::<HighlightDocument>(&job_dir.join("timeline").join("highlights.json"))
            .map_err(|_| "高光候选数据不完整，请重新分析录像。".to_string())?;
    let timeline =
        read_json::<TimelineDocument>(&job_dir.join("timeline").join("combat-events.json"))
            .map_err(|_| "回放阵容数据不完整，请重新分析录像。".to_string())?;

    let mut seen = HashSet::new();
    let mut clips = Vec::with_capacity(request.clips.len());
    let mut total_duration_seconds = 0.0;

    for clip in request.clips {
        if !valid_clip_id(&clip.clip_id) {
            return Err("剪辑方案包含无效的片段编号。".to_string());
        }
        if !seen.insert(clip.clip_id.clone()) {
            return Err("剪辑方案包含重复的片段编号。".to_string());
        }
        let Some(candidate) = highlights
            .candidates
            .iter()
            .find(|candidate| candidate.id == clip.candidate_id)
        else {
            return Err(format!("找不到候选片段 {}。", clip.candidate_id));
        };
        if !clip.source_start_seconds.is_finite()
            || !clip.source_end_seconds.is_finite()
            || clip.source_start_seconds < 0.0
            || clip.source_end_seconds <= clip.source_start_seconds
            || clip.source_end_seconds > timeline.replay.playback_time_seconds
        {
            return Err(format!("候选 {} 的起止时间无效。", clip.candidate_id));
        }
        let duration = clip.source_end_seconds - clip.source_start_seconds;
        if !(1.0..=90.0).contains(&duration) {
            return Err(format!(
                "片段 {} 的时长必须在 1 到 90 秒之间。",
                clip.clip_id
            ));
        }
        if let Some(hero) = clip.view_hero.as_deref() {
            let valid_hero = if timeline.replay.players.is_empty() {
                candidate.participants.iter().any(|value| value == hero)
                    || candidate.primary_hero.as_deref() == Some(hero)
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

        total_duration_seconds += clip.source_end_seconds - clip.source_start_seconds;
        clips.push(EditPlanClip {
            clip_id: clip.clip_id,
            candidate_id: clip.candidate_id,
            view_hero: clip.view_hero,
            camera_mode: normalize_user_camera_mode(clip.camera_mode),
            source_start_seconds: clip.source_start_seconds,
            source_end_seconds: clip.source_end_seconds,
        });
    }

    let document = EditPlanDocument {
        schema_version: EDIT_PLAN_SCHEMA_VERSION.to_string(),
        source_sha256: manifest.source.sha256,
        job_id: request.job_id,
        mode: "manual".to_string(),
        updated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs(),
        clips,
        settings: manual_render_settings(request.settings),
    };
    let director_dir = job_dir.join("director");
    fs::create_dir_all(&director_dir).map_err(|error| error.to_string())?;
    let output_path = director_dir.join("edit-plan.json");
    let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    fs::write(output_path, bytes).map_err(|error| error.to_string())?;

    Ok(SaveEditPlanResult {
        selected_clip_count: document.clips.len(),
        total_duration_seconds,
    })
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
        .join(job_id)
        .join("director")
        .join("edit-plan.json");
    if !path.is_file() {
        return Ok(None);
    }

    let document = read_json::<EditPlanDocument>(&path)?;
    Ok(load_edit_plan(document))
}

fn load_edit_plan(document: EditPlanDocument) -> Option<LoadedEditPlan> {
    if document.schema_version != EDIT_PLAN_SCHEMA_VERSION {
        return None;
    }
    Some(LoadedEditPlan {
        mode: document.mode,
        clips: document
            .clips
            .into_iter()
            .enumerate()
            .map(|(index, clip)| LoadedEditPlanClip {
                clip_id: if clip.clip_id.is_empty() {
                    format!("clip-saved-{:02}", index + 1)
                } else {
                    clip.clip_id
                },
                candidate_id: clip.candidate_id,
                view_hero: clip.view_hero,
                camera_mode: normalize_user_camera_mode(clip.camera_mode),
                source_start_seconds: clip.source_start_seconds,
                source_end_seconds: clip.source_end_seconds,
            })
            .collect(),
        settings: document.settings,
    })
}

#[tauri::command]
fn get_latest_render(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<Option<RenderResult>, String> {
    if !valid_job_id(&job_id) {
        return Err("任务编号无效，请重新打开分析结果。".to_string());
    }
    let job_dir = state.jobs_root.join(&job_id);
    let output_dir = job_dir.join("output");
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
    let segment_count =
        read_json::<EditPlanDocument>(&job_dir.join("director").join("edit-plan.json"))
            .map(|plan| plan.clips.len())
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

        return Ok(Some(RenderResult {
            output_path: output_path.display().to_string(),
            qc_report_path: report_path.display().to_string(),
            duration_seconds: report.duration_seconds,
            width: report.width,
            height: report.height,
            segment_count,
            warnings: report.warnings,
        }));
    }

    Ok(None)
}

#[tauri::command]
async fn start_render(
    job_id: String,
    on_progress: Channel<RenderProgress>,
    state: State<'_, AppState>,
) -> Result<RenderResult, String> {
    if !valid_job_id(&job_id) {
        return Err("任务编号无效，请重新打开分析结果。".to_string());
    }
    let cancellation = {
        let mut runtime = state
            .render_runtime
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

    let request = build_render_request(&job_id, &state);
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            reset_render_runtime(&state.render_runtime);
            return Err(error);
        }
    };
    let progress_channel = on_progress.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let result = render(request, cancellation, |progress| {
            let _ = progress_channel.send(progress);
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
    reset_render_runtime(&state.render_runtime);
    joined.map_err(|error| error.to_string())?
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
fn open_local_path(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let target = fs::canonicalize(PathBuf::from(path))
        .map_err(|_| "输出文件不存在，可能已被移动。".to_string())?;
    let jobs_root = fs::canonicalize(&state.jobs_root).map_err(|error| error.to_string())?;
    if !target.starts_with(&jobs_root) {
        return Err("只能打开猫猫任务目录中的输出文件。".to_string());
    }
    let mut command = if target.is_dir() {
        let mut command = Command::new("explorer.exe");
        command.arg(&target);
        command
    } else {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler").arg(&target);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开输出：{error}"))
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
    let edit_plan = read_json::<EditPlanDocument>(&job_dir.join("director").join("edit-plan.json"))
        .map_err(|_| "请先保存剪辑方案，再开始生成成片。".to_string())?;
    if edit_plan.source_sha256 != manifest.source.sha256
        || timeline.source_sha256 != manifest.source.sha256
        || highlights.source_sha256 != manifest.source.sha256
    {
        return Err("任务数据版本不一致，请重新分析录像。".to_string());
    }

    let mut clips = Vec::with_capacity(edit_plan.clips.len());
    for clip in &edit_plan.clips {
        let candidate = highlights
            .candidates
            .iter()
            .find(|candidate| candidate.id == clip.candidate_id)
            .ok_or_else(|| format!("找不到候选片段 {}。", clip.candidate_id))?;
        let inset = ((clip.source_end_seconds - clip.source_start_seconds) * 0.1).min(0.25);
        let style_peak = candidate.peak_seconds.clamp(
            clip.source_start_seconds + inset,
            clip.source_end_seconds - inset,
        );
        clips.push(RenderClip {
            clip_id: clip.clip_id.clone(),
            candidate_id: clip.candidate_id.clone(),
            view_hero: clip
                .view_hero
                .clone()
                .or_else(|| candidate.primary_hero.clone()),
            camera_mode: normalize_user_camera_mode(clip.camera_mode.clone()),
            source_start_seconds: clip.source_start_seconds,
            source_peak_seconds: style_peak,
            source_end_seconds: clip.source_end_seconds,
            anchor_seconds: candidate.peak_seconds,
            anchor_tick: candidate.anchor_tick,
        });
    }

    Ok(RenderRequest {
        job_id: job_id.to_string(),
        source_sha256: manifest.source.sha256,
        job_dir,
        source_replay: PathBuf::from(manifest.source.path),
        dota2_exe: state
            .dota2_exe
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
        settings: manual_render_settings(edit_plan.settings),
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

fn source_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = project_root();
            let executable_dir = env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| root.clone());
            let resource_dir = app.path().resource_dir()?;
            let jobs_root =
                if let Some(configured_root) = env::var_os("D2H_PROJECT_ROOT").map(PathBuf::from) {
                    configured_root.join("jobs")
                } else if executable_dir.join("Cargo.toml").is_file()
                    || executable_dir.join("jobs").is_dir()
                {
                    executable_dir.join("jobs")
                } else if cfg!(debug_assertions) {
                    root.join("jobs")
                } else {
                    app.path().app_local_data_dir()?.join("jobs")
                };
            fs::create_dir_all(&jobs_root)?;
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
            app.manage(AppState {
                jobs_root,
                dota2_exe: find_dota2(),
                ffmpeg_exe,
                ffprobe_exe,
                render_runtime: Arc::new(Mutex::new(RenderRuntime::default())),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_capabilities,
            resolve_replay_by_id,
            get_recent_jobs,
            analyze_dem,
            save_edit_plan,
            get_edit_plan,
            get_latest_render,
            start_render,
            cancel_render,
            open_local_path
        ])
        .run(tauri::generate_context!())
        .expect("unable to start Cat Cut Assistant");
}

#[cfg(test)]
mod tests {
    use super::{
        EditPlanDocument, load_edit_plan, manual_render_settings, normalize_user_camera_mode,
        replay_directory_from_dota2, resolve_replay_by_id, valid_clip_id, valid_job_id,
        valid_replay_id,
    };
    use d2_highlights_renderer::{BgmMode, CameraStyle, ClipCameraMode, RenderSettings};
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn edit_plan_job_id_stays_inside_jobs_root() {
        assert!(valid_job_id("d2h-ff5145119d7415b3"));
        assert!(!valid_job_id("../d2h-ff5145119d7415b3"));
        assert!(!valid_job_id("d2h-.."));
        assert!(!valid_job_id("other-job"));
    }

    #[test]
    fn clip_ids_are_stable_and_path_safe() {
        assert!(valid_clip_id("clip-recommended-01"));
        assert!(valid_clip_id("clip-example-uuid"));
        assert!(!valid_clip_id("../clip-01"));
        assert!(!valid_clip_id("hl-001"));
    }

    #[test]
    fn replay_ids_are_digits_only_and_path_safe() {
        assert!(valid_replay_id("123456789"));
        assert!(valid_replay_id("1"));
        assert!(!valid_replay_id(""));
        assert!(!valid_replay_id("123456789.dem"));
        assert!(!valid_replay_id("../123456789"));
        assert!(!valid_replay_id(&"1".repeat(21)));
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
    fn replay_lookup_finds_a_valid_source2_dem() {
        let directory = tempdir().expect("create replay directory");
        let replay_path = directory.path().join("123456789.dem");
        let mut replay = fs::File::create(&replay_path).expect("create replay");
        replay.write_all(b"PBDEMS2\0fixture").expect("write replay");

        let result = resolve_replay_by_id(
            directory.path().display().to_string(),
            "123456789".to_string(),
        )
        .expect("resolve replay");

        assert_eq!(result.replay_id, "123456789");
        assert_eq!(Path::new(&result.path), replay_path);
    }

    #[test]
    fn replay_lookup_reports_a_missing_number() {
        let directory = tempdir().expect("create replay directory");
        let error = resolve_replay_by_id(
            directory.path().display().to_string(),
            "987654321".to_string(),
        )
        .expect_err("missing replay must fail");
        assert!(error.contains("987654321.dem"));
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
    fn stale_edit_plan_does_not_override_new_detector_results() {
        let stale = EditPlanDocument {
            schema_version: "d2h.edit-plan/1.2".to_string(),
            source_sha256: "source".to_string(),
            job_id: "d2h-aaaaaaaaaaaaaaaa".to_string(),
            mode: "manual".to_string(),
            updated_unix_seconds: 0,
            clips: Vec::new(),
            settings: RenderSettings::default(),
        };

        assert!(load_edit_plan(stale).is_none());
    }
}
