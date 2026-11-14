mod audio;
mod cache_storage;
mod capture_files;
pub use cache_storage::{CacheCleanupSummary, clean_render_cache, inspect_render_cache};
mod execution;
mod resume;
pub use resume::{RenderResumeSummary, inspect_resume};

use audio::write_original_bgm;
use capture_files::CaptureFiles;
use d2_highlights_core::TimelineDocument;
use d2_highlights_replay_control::{
    DEMO_STATUS_COMMAND, ReplayControlError, VConsoleExecutionReport, VConsoleSession,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
#[cfg(windows)]
use std::sync::Once;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
};

pub const RENDER_SCHEMA_VERSION: &str = "d2h.render/1.9";
// Bump this whenever Dota capture commands or frame/audio encoding behavior changes.
const CAPTURE_PIPELINE_VERSION: &str = "d2h.capture/2.3.8-selected-hero-switch-17";
// Native capture/output cadence is independent of the 30 Hz editor time grid.
const FRAME_RATE: u32 = 60;
const NATIVE_JPEG_QUALITY: u32 = 100;
const VIDEO_CRF: &str = "16";
// Preserve the game's resolution. Convert JPEG YCbCr to a standard HD video
// matrix/range at high precision; do not apply brightness or gamma filters.
const NATIVE_VIDEO_FILTER: &str = "pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black,zscale=matrixin=470bg:matrix=709:rangein=full:range=limited:dither=error_diffusion,format=yuv420p,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709";
const CAPTURE_PREROLL_SECONDS: f32 = 1.0;
const CAMERA_REASSERT_CAPTURE_FRAMES: usize = FRAME_RATE as usize / 2;
const CAPTURE_PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);
const VCONSOLE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const STATUS_DLL_INIT_FAILED: i32 = 0xC000_0142_u32 as i32;

fn background_command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(windows)]
    configure_child_process_error_mode();
    let mut command = Command::new(program);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(windows)]
fn configure_child_process_error_mode() {
    static CONFIGURE_ONCE: Once = Once::new();
    CONFIGURE_ONCE.call_once(|| {
        // Child processes inherit this mode, so loader failures return to the app instead of
        // interrupting an unattended render with a Windows modal error dialog.
        unsafe {
            SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
        }
    });
}

fn output_with_loader_retry(mut build: impl FnMut() -> Command) -> io::Result<Output> {
    let first = execution::output(build())?;
    if is_dll_init_failure(first.status.code()) {
        thread::sleep(Duration::from_millis(300));
        execution::output(build())
    } else {
        Ok(first)
    }
}

fn is_dll_init_failure(exit_code: Option<i32>) -> bool {
    #[cfg(windows)]
    {
        exit_code == Some(STATUS_DLL_INIT_FAILED)
    }
    #[cfg(not(windows))]
    {
        let _ = exit_code;
        false
    }
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CameraStyle {
    AutoDirector,
    HeroFocus,
    TacticalOverview,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ClipCameraMode {
    Directed,
    FreeCamera,
    HeroChase,
    #[default]
    PlayerPerspective,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderTakeRole {
    #[default]
    Primary,
    Alternate,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BgmMode {
    Original,
    Custom,
    GameOnly,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RenderSettings {
    pub camera_style: CameraStyle,
    pub clean_hud: bool,
    pub slow_motion: bool,
    pub replay_emphasis: bool,
    pub bgm_mode: BgmMode,
    pub custom_bgm_path: Option<String>,
    pub game_audio_volume: f32,
    pub bgm_volume: f32,
    pub impact_sfx: bool,
    pub system_narration: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            camera_style: CameraStyle::AutoDirector,
            clean_hud: true,
            slow_motion: false,
            replay_emphasis: false,
            bgm_mode: BgmMode::GameOnly,
            custom_bgm_path: None,
            game_audio_volume: 1.0,
            bgm_volume: 0.0,
            impact_sfx: false,
            system_narration: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderClip {
    pub clip_id: String,
    pub candidate_id: String,
    pub view_hero: Option<String>,
    #[serde(default)]
    pub camera_mode: ClipCameraMode,
    #[serde(default)]
    pub take_group_id: Option<String>,
    #[serde(default)]
    pub take_role: RenderTakeRole,
    #[serde(default = "default_true")]
    pub include_in_final: bool,
    pub source_start_seconds: f32,
    pub source_peak_seconds: f32,
    pub source_end_seconds: f32,
    pub anchor_seconds: f32,
    pub anchor_tick: u32,
}

#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub job_id: String,
    pub source_sha256: String,
    pub job_dir: PathBuf,
    pub source_replay: PathBuf,
    pub dota2_exe: PathBuf,
    pub ffmpeg_exe: PathBuf,
    pub ffprobe_exe: PathBuf,
    pub timeline: TimelineDocument,
    pub clips: Vec<RenderClip>,
    pub settings: RenderSettings,
    // Explicit UI choice, excluded from the media/plan fingerprint.
    pub resume_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderProgress {
    pub stage: String,
    pub status: String,
    pub message: String,
    pub percent: u8,
    pub current_clip: usize,
    pub total_clips: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderResult {
    pub output_path: String,
    pub qc_report_path: String,
    pub source_assets_dir: String,
    pub source_assets: Vec<RenderSourceAsset>,
    pub duration_seconds: f32,
    pub width: u32,
    pub height: u32,
    pub segment_count: usize,
    pub source_asset_count: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RenderSourceAsset {
    pub asset_id: String,
    pub scene_number: usize,
    pub take_index: usize,
    pub clip_id: String,
    pub take_group_id: Option<String>,
    pub take_role: RenderTakeRole,
    pub included_in_final: bool,
    pub output_path: String,
    pub view_hero: Option<String>,
    pub camera_mode: ClipCameraMode,
    pub source_start_seconds: f32,
    pub source_end_seconds: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SegmentState {
    candidate_id: String,
    output_path: String,
    duration_seconds: f32,
    status: String,
    #[serde(default)]
    capture_pipeline: String,
    #[serde(default)]
    content_sha256: String,
    #[serde(default)]
    quality_checked: bool,
    #[serde(default)]
    camera_target_verified: bool,
    #[serde(default)]
    expected_duration_seconds: f32,
}

#[derive(Clone, Debug, Serialize)]
struct CaptureDiagnostics {
    clip_id: String,
    requested_camera: ClipCameraMode,
    requested_hero: Option<String>,
    requested_slot: Option<u8>,
    requested_tick: u32,
    observed_tick: u32,
    observed_look_at: [f32; 3],
    camera_target_verified: bool,
    verification_status: String,
    expected_frames: usize,
    captured_frames: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RenderState {
    schema_version: String,
    fingerprint: String,
    updated_unix_seconds: u64,
    segments: BTreeMap<String, SegmentState>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct QcReport {
    schema_version: String,
    output_path: String,
    duration_seconds: f32,
    width: u32,
    height: u32,
    has_video: bool,
    has_audio: bool,
    black_events: usize,
    freeze_events: usize,
    audio_mean_db: Option<f32>,
    audio_peak_db: Option<f32>,
    #[serde(default)]
    final_segment_count: usize,
    #[serde(default)]
    source_assets_dir: String,
    #[serde(default)]
    source_assets: Vec<RenderSourceAsset>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceAssetManifest<'a> {
    schema_version: &'static str,
    job_id: &'a str,
    source_sha256: &'a str,
    assets: &'a [RenderSourceAsset],
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("成片任务已取消。")]
    Cancelled,
    #[error("Dota 2 已在运行。请先正常关闭客户端，再开始离线成片。")]
    DotaAlreadyRunning,
    #[error(
        "Steam 未运行。请手动打开并登录 Steam，保持运行后重试录制；本软件不会自动启动或登录 Steam。"
    )]
    SteamRequiredForRecording,
    #[error("找不到或无法运行 {0}。")]
    MissingTool(String),
    #[error("录像文件不存在：{0}")]
    MissingReplay(String),
    #[error("剪辑方案无效：{0}")]
    InvalidPlan(String),
    #[error("Dota 2 启动后未能建立离线控制连接。\n最近一次反馈：{0}")]
    VConsoleUnavailable(String),
    #[error("Dota 2 在离线导出期间意外退出。")]
    DotaExited,
    #[error("Dota 2 回放控制失败：{0}")]
    ReplayControl(String),
    #[error("Dota 2 没有输出片段 {0} 的有效帧序列。")]
    MissingFrames(String),
    #[error("媒体处理失败：{0}")]
    Media(String),
    #[error("文件操作失败：{0}")]
    Io(#[from] io::Error),
    #[error("任务数据失败：{0}")]
    Json(#[from] serde_json::Error),
    #[error("系统时间无效。")]
    Clock,
}

pub fn render<F>(
    request: RenderRequest,
    cancellation: CancellationToken,
    on_progress: F,
) -> Result<RenderResult, RenderError>
where
    F: FnMut(RenderProgress),
{
    let _storage_lease = cache_storage::lock_job_storage(&request.job_dir)?;
    let log_path = request
        .job_dir
        .join("render")
        .join("logs")
        .join(format!("render-{}.jsonl", unique_stamp()));
    let _scope = execution::Scope::enter(cancellation.clone(), log_path.clone())?;
    execution::record(
        "render_start",
        serde_json::json!({
            "jobId": request.job_id, "capturePipeline": CAPTURE_PIPELINE_VERSION,
            "clips": request.clips, "diagnostics": log_path,
            "resumeFingerprint": request.resume_fingerprint,
        }),
    )?;
    let mut result = render_inner(request, cancellation.clone(), on_progress);
    if cancellation.is_cancelled() {
        result = Err(RenderError::Cancelled);
    }
    let final_log_result = execution::record(
        "render_finished",
        serde_json::json!({
            "success": result.is_ok(), "error": result.as_ref().err().map(ToString::to_string),
        }),
    );
    if result.is_ok() {
        final_log_result?;
    }
    result
}

fn render_inner<F>(
    request: RenderRequest,
    cancellation: CancellationToken,
    mut on_progress: F,
) -> Result<RenderResult, RenderError>
where
    F: FnMut(RenderProgress),
{
    validate_request(&request)?;
    emit(
        &mut on_progress,
        "preflight",
        "running",
        "正在检查已保存素材、编码器和剪辑方案",
        2,
        0,
        request.clips.len(),
    );
    check_cancelled(&cancellation)?;
    check_command(&request.ffmpeg_exe, &["-version"], "FFmpeg")?;
    check_command(&request.ffprobe_exe, &["-version"], "FFprobe")?;
    let fingerprint = render_fingerprint(&request)?;
    let should_resume = resume::resume_requested(&request, &fingerprint)?;
    let cache_root = request
        .job_dir
        .join("render")
        .join("cache")
        .join(&fingerprint[..16]);
    let segment_root = cache_root.join("segments");
    let state_path = cache_root.join("state.json");
    fs::create_dir_all(&segment_root)?;
    let mut state = load_or_create_state(&state_path, &fingerprint)?;
    let saved = if should_resume {
        emit(
            &mut on_progress,
            "preflight",
            "running",
            "正在核验断点片段，完整素材将直接复用",
            3,
            0,
            request.clips.len(),
        );
        resume::verified_segments(&request, &state, &segment_root, &cancellation, true)?
    } else {
        BTreeMap::new()
    };
    execution::record(
        "resume_plan",
        serde_json::json!({"requested": should_resume, "completedClips": saved.len(), "remainingClips": request.clips.len() - saved.len()}),
    )?;

    let segments = if saved.len() == request.clips.len() {
        let mut segments = Vec::with_capacity(request.clips.len());
        for (index, clip) in request.clips.iter().enumerate() {
            check_cancelled(&cancellation)?;
            segments.push(resume::reused_clip(
                clip,
                index,
                request.clips.len(),
                &saved[&clip.clip_id],
                &mut on_progress,
            )?);
        }
        segments
    } else {
        validate_recording_clients(dota_is_running()?, steam_is_running()?)?;
        let movie_dir = dota_game_dir(&request.dota2_exe)?
            .join("dota")
            .join("movie");
        fs::create_dir_all(&movie_dir)?;

        let replay_dir = dota_game_dir(&request.dota2_exe)?
            .join("dota")
            .join("replays");
        fs::create_dir_all(&replay_dir)?;
        let prepared_replay = prepare_replay(
            &request.source_replay,
            &replay_dir,
            &request.job_id,
            &request.source_sha256,
        )?;

        emit(
            &mut on_progress,
            "launch",
            "running",
            "正在启动 Dota 2 离线回放环境",
            6,
            0,
            request.clips.len(),
        );
        let mut owned_dota = OwnedDota(launch_dota(&request.dota2_exe)?, None);
        // From successful spawn until verified shutdown there is no early return,
        // including a diagnostic-write failure or a cancelled capture session.
        let session_result = (|| {
            execution::record(
                "dota_started",
                serde_json::json!({"pid": owned_dota.0.id()}),
            )?;
            run_dota_session(
                &request,
                &prepared_replay.replay_reference,
                &movie_dir,
                &segment_root,
                &state_path,
                &mut state,
                &saved,
                &cancellation,
                &mut owned_dota.0,
                &mut owned_dota.1,
                &mut on_progress,
            )
        })();
        finish_dota_session(
            &mut owned_dota.0,
            &mut owned_dota.1,
            request.clips.len(),
            &mut on_progress,
        );

        session_result?
    };
    let mut warnings = Vec::new();
    let unverified_count = request
        .clips
        .iter()
        .filter(|clip| {
            state
                .segments
                .get(&clip.clip_id)
                .is_none_or(|segment| !segment.camera_target_verified)
        })
        .count();
    if unverified_count > 0 {
        warnings.push(format!(
            "{unverified_count} 段素材已检查回放 tick、镜头位置和媒体完整性，但尚未核验画面中的目标英雄/事件；请查看各段诊断并确认画面。续录只会在你选择继续后复用完整片段，不把复用视为自动识别英雄成功。"
        ));
    }
    check_cancelled(&cancellation)?;

    emit(
        &mut on_progress,
        "edit",
        "running",
        "正在整理编号素材并组合默认成片",
        82,
        request.clips.len(),
        request.clips.len(),
    );
    let output_dir = request.job_dir.join("output");
    fs::create_dir_all(&output_dir)?;
    let replay_stem = replay_output_stem(&request.source_replay);
    let output_stem = available_output_stem(&output_dir, &replay_stem);
    let source_assets_dir = output_dir.join(format!("{output_stem}_源素材"));
    let source_assets = export_source_assets(&request, &segments, &source_assets_dir)?;
    let final_segments = select_final_segments(&request.clips, &segments)?;
    let final_segment_paths = final_segments
        .iter()
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    let joined_path = cache_root.join("joined.mp4");
    concat_segments(
        &request.ffmpeg_exe,
        &final_segment_paths,
        &cache_root,
        &joined_path,
    )?;
    let joined_duration = probe_media(&request.ffprobe_exe, &joined_path)?.duration_seconds;
    let impact_cues = impact_cues(&request.ffprobe_exe, &final_segments);

    let narration_path = if request.settings.system_narration {
        let path = cache_root.join("narration.wav");
        match write_system_narration(
            &path,
            &format!(
                "猫猫为你挑出了{}段精彩高光。现在开始。",
                final_segments.len()
            ),
        ) {
            Ok(()) => Some(path),
            Err(error) => {
                warnings.push(format!("系统旁白不可用，已继续生成无旁白版本：{error}"));
                None
            }
        }
    } else {
        None
    };

    let bgm_path = match request.settings.bgm_mode {
        BgmMode::Original => {
            let path = cache_root.join("cat-original-bgm.wav");
            write_original_bgm(
                &path,
                joined_duration + 1.0,
                if request.settings.impact_sfx {
                    &impact_cues
                } else {
                    &[]
                },
            )?;
            Some(path)
        }
        BgmMode::Custom => {
            let path = request
                .settings
                .custom_bgm_path
                .as_deref()
                .map(PathBuf::from)
                .filter(|path| path.is_file())
                .ok_or_else(|| {
                    RenderError::InvalidPlan("自选 BGM 文件不存在或无法读取。".to_string())
                })?;
            Some(path)
        }
        BgmMode::GameOnly => None,
    };

    check_cancelled(&cancellation)?;
    let final_path = output_dir.join(format!("{output_stem}.mp4"));
    let pending_final_path =
        output_dir.join(format!("{output_stem}.partial-{}.mp4", unique_stamp()));
    mix_final_audio(
        &request.ffmpeg_exe,
        &joined_path,
        bgm_path.as_deref(),
        narration_path.as_deref(),
        &pending_final_path,
        joined_duration,
        &request.settings,
    )?;

    emit(
        &mut on_progress,
        "qc",
        "running",
        "正在检查分辨率、音轨、黑帧和冻结画面",
        94,
        request.clips.len(),
        request.clips.len(),
    );
    check_cancelled(&cancellation)?;
    let mut qc = run_qc(
        &request.ffmpeg_exe,
        &request.ffprobe_exe,
        &pending_final_path,
    )?;
    check_cancelled(&cancellation)?;
    fs::rename(&pending_final_path, &final_path)?;
    qc.output_path = final_path.display().to_string();
    warnings.append(&mut qc.warnings);
    qc.final_segment_count = final_segments.len();
    qc.source_assets_dir = source_assets_dir.display().to_string();
    qc.source_assets = source_assets.clone();
    qc.warnings = warnings.clone();
    let qc_report_path = output_dir.join(format!("{output_stem}_质量报告.json"));
    write_json(&qc_report_path, &qc)?;

    emit(
        &mut on_progress,
        "complete",
        "complete",
        if saved.len() == request.clips.len() {
            "成片已完成，已复用全部片段"
        } else {
            "成片已完成，Dota 2 已关闭"
        },
        100,
        request.clips.len(),
        request.clips.len(),
    );
    Ok(RenderResult {
        output_path: final_path.display().to_string(),
        qc_report_path: qc_report_path.display().to_string(),
        duration_seconds: qc.duration_seconds,
        width: qc.width,
        height: qc.height,
        segment_count: final_segments.len(),
        source_assets_dir: source_assets_dir.display().to_string(),
        source_asset_count: source_assets.len(),
        source_assets,
        warnings,
    })
}

fn replay_output_stem(source_replay: &Path) -> String {
    let stem = source_replay
        .file_stem()
        .map(|value| value.to_string_lossy())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "录像".into());
    stem.chars().take(160).collect()
}

fn available_output_stem(output_dir: &Path, replay_stem: &str) -> String {
    let mut sequence = 1_u32;
    loop {
        let candidate = if sequence == 1 {
            replay_stem.to_string()
        } else {
            format!("{replay_stem}_{sequence}")
        };
        let occupied = output_dir.join(format!("{candidate}.mp4")).exists()
            || output_dir
                .join(format!("{candidate}_质量报告.json"))
                .exists()
            || output_dir.join(format!("{candidate}_源素材")).exists();
        if !occupied {
            return candidate;
        }
        sequence = sequence.saturating_add(1);
    }
}

fn select_final_segments(
    clips: &[RenderClip],
    segments: &[PathBuf],
) -> Result<Vec<(RenderClip, PathBuf)>, RenderError> {
    if clips.len() != segments.len() {
        return Err(RenderError::Media(
            "导出的源素材数量与剪辑方案不一致。".to_string(),
        ));
    }
    let selected = clips
        .iter()
        .zip(segments)
        .filter(|(clip, _)| clip.include_in_final)
        .map(|(clip, path)| (clip.clone(), path.clone()))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(RenderError::InvalidPlan(
            "请至少保留一个默认入片的主机位素材。".to_string(),
        ));
    }
    Ok(selected)
}

fn export_source_assets(
    request: &RenderRequest,
    segments: &[PathBuf],
    output_dir: &Path,
) -> Result<Vec<RenderSourceAsset>, RenderError> {
    if request.clips.len() != segments.len() {
        return Err(RenderError::Media(
            "导出的源素材数量与剪辑方案不一致。".to_string(),
        ));
    }
    fs::create_dir_all(output_dir)?;
    let numbering = source_asset_numbering(&request.clips);
    let mut assets = Vec::with_capacity(request.clips.len());
    for ((clip, segment), (asset_id, scene_number, take_index)) in
        request.clips.iter().zip(segments).zip(numbering)
    {
        let output_path = output_dir.join(format!(
            "{}_{}.mp4",
            asset_id,
            camera_asset_label(&clip.camera_mode)
        ));
        cache_storage::link_or_copy_new(segment, &output_path)?;
        assets.push(RenderSourceAsset {
            asset_id,
            scene_number,
            take_index,
            clip_id: clip.clip_id.clone(),
            take_group_id: clip.take_group_id.clone(),
            take_role: clip.take_role,
            included_in_final: clip.include_in_final,
            output_path: output_path.display().to_string(),
            view_hero: clip.view_hero.clone(),
            camera_mode: clip.camera_mode.clone(),
            source_start_seconds: clip.source_start_seconds,
            source_end_seconds: clip.source_end_seconds,
        });
    }
    let manifest = SourceAssetManifest {
        schema_version: RENDER_SCHEMA_VERSION,
        job_id: &request.job_id,
        source_sha256: &request.source_sha256,
        assets: &assets,
    };
    write_json(&output_dir.join("素材清单.json"), &manifest)?;
    Ok(assets)
}

fn source_asset_numbering(clips: &[RenderClip]) -> Vec<(String, usize, usize)> {
    let mut scene_by_group = HashMap::<String, usize>::new();
    let mut grouped_indices = Vec::<Vec<usize>>::new();
    for (index, clip) in clips.iter().enumerate() {
        let key = clip
            .take_group_id
            .clone()
            .unwrap_or_else(|| format!("standalone:{}", clip.clip_id));
        let scene_index = match scene_by_group.get(&key).copied() {
            Some(scene_index) => scene_index,
            None => {
                let scene_index = grouped_indices.len();
                grouped_indices.push(Vec::new());
                scene_by_group.insert(key, scene_index);
                scene_index
            }
        };
        grouped_indices[scene_index].push(index);
    }

    let mut numbering = vec![(String::new(), 0, 0); clips.len()];
    for (scene_index, indices) in grouped_indices.into_iter().enumerate() {
        let mut ordered = indices
            .iter()
            .copied()
            .filter(|index| clips[*index].take_role == RenderTakeRole::Primary)
            .collect::<Vec<_>>();
        ordered.extend(
            indices
                .iter()
                .copied()
                .filter(|index| clips[*index].take_role == RenderTakeRole::Alternate),
        );
        for (take_index, clip_index) in ordered.into_iter().enumerate() {
            let scene_number = scene_index + 1;
            numbering[clip_index] = (
                format!("S{scene_number:03}-{}", take_code(take_index)),
                scene_number,
                take_index + 1,
            );
        }
    }
    numbering
}

fn take_code(index: usize) -> String {
    let mut value = index + 1;
    let mut code = String::new();
    while value > 0 {
        value -= 1;
        code.insert(0, (b'A' + (value % 26) as u8) as char);
        value /= 26;
    }
    code
}

fn camera_asset_label(mode: &ClipCameraMode) -> &'static str {
    match mode {
        ClipCameraMode::PlayerPerspective => "玩家视角",
        ClipCameraMode::HeroChase => "英雄跟随",
        ClipCameraMode::Directed => "导播视角",
        ClipCameraMode::FreeCamera => "自由视角",
    }
}

#[allow(clippy::too_many_arguments)]
fn run_dota_session<F>(
    request: &RenderRequest,
    replay_reference: &str,
    movie_dir: &Path,
    segment_root: &Path,
    state_path: &Path,
    state: &mut RenderState,
    saved: &BTreeMap<String, PathBuf>,
    cancellation: &CancellationToken,
    child: &mut Child,
    control: &mut Option<VConsoleSession>,
    on_progress: &mut F,
) -> Result<Vec<PathBuf>, RenderError>
where
    F: FnMut(RenderProgress),
{
    wait_for_vconsole(child, control, cancellation, |elapsed_seconds| {
        emit(
            on_progress,
            "launch",
            "running",
            &format!("正在等待游戏控制连接（已等待 {elapsed_seconds} 秒）"),
            6,
            0,
            request.clips.len(),
        );
    })?;
    let control = control.as_mut().expect("VConsole handshake completed");
    check_cancelled(cancellation)?;
    emit(
        on_progress,
        "launch",
        "running",
        "离线控制已连接，正在载入录像",
        8,
        0,
        request.clips.len(),
    );
    send_commands(control, &[format!("playdemo {replay_reference}")])?;
    wait_with_cancel(
        Duration::from_secs(12),
        cancellation,
        child,
        control,
        |_| {},
    )?;

    let total = request.clips.len();
    let mut rendered_segments = Vec::with_capacity(total);
    for (index, clip) in request.clips.iter().enumerate() {
        check_cancelled(cancellation)?;
        if let Some(path) = saved.get(&clip.clip_id) {
            rendered_segments.push(resume::reused_clip(clip, index, total, path, on_progress)?);
            continue;
        }
        execution::record(
            "capture_begin",
            serde_json::json!({
                "clipId": clip.clip_id, "sourceStart": clip.source_start_seconds,
                "sourceEnd": clip.source_end_seconds, "camera": clip.camera_mode,
                "hero": clip.view_hero, "cacheDecision": "missing_or_not_verified",
            }),
        )?;
        let output_path = segment_root.join(format!(
            "{:02}-{}-{}.mp4",
            index + 1,
            clip.clip_id,
            unique_stamp()
        ));
        let pending_output = output_path.with_extension("partial.mp4");

        emit(
            on_progress,
            "capture",
            "running",
            &format!("正在导出第 {}/{} 段：{}", index + 1, total, clip.clip_id),
            clip_percent(index, total),
            index + 1,
            total,
        );
        let clip_dir = segment_root.join(format!(
            "{:02}-{}-{}",
            index + 1,
            clip.clip_id,
            unique_stamp()
        ));
        fs::create_dir_all(&clip_dir)?;
        let base_path = clip_dir.join("base.mp4");
        let diagnostics = capture_clip(
            request,
            clip,
            index,
            movie_dir,
            &clip_dir,
            &base_path,
            cancellation,
            child,
            control,
            |elapsed, total_duration| {
                let fraction = if total_duration > 0.0 {
                    (elapsed / total_duration).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let overall = 10.0 + ((index as f32 + fraction) / total as f32 * 68.0).round();
                emit(
                    on_progress,
                    "capture",
                    "running",
                    &format!(
                        "第 {}/{} 段 · 已保存 {:.1}/{:.1} 秒 · {FRAME_RATE} FPS",
                        index + 1,
                        total,
                        elapsed,
                        total_duration
                    ),
                    overall.clamp(0.0, 100.0) as u8,
                    index + 1,
                    total,
                );
            },
        )?;
        apply_replay_emphasis(
            &request.ffmpeg_exe,
            &request.ffprobe_exe,
            &base_path,
            &pending_output,
            clip,
            request.settings.replay_emphasis,
        )?;
        check_cancelled(cancellation)?;
        let media = probe_media(&request.ffprobe_exe, &pending_output)?;
        let expected_duration = expected_clip_duration(clip, &request.settings);
        validate_segment_media(&media, expected_duration)?;
        let segment_qc = run_qc(&request.ffmpeg_exe, &request.ffprobe_exe, &pending_output)?;
        write_json(&clip_dir.join("quality.json"), &segment_qc)?;
        let content_sha256 = file_sha256(&pending_output)?;
        check_cancelled(cancellation)?;
        fs::rename(&pending_output, &output_path)?;
        let quality_checked = segment_qc.warnings.is_empty();
        state.segments.insert(
            clip.clip_id.clone(),
            SegmentState {
                candidate_id: clip.candidate_id.clone(),
                output_path: output_path.display().to_string(),
                duration_seconds: media.duration_seconds,
                // Media completion does not imply automatic hero identification.
                status: if quality_checked {
                    "complete"
                } else {
                    "quality_warning"
                }
                .to_string(),
                capture_pipeline: CAPTURE_PIPELINE_VERSION.to_string(),
                content_sha256,
                quality_checked,
                camera_target_verified: diagnostics.camera_target_verified,
                expected_duration_seconds: expected_duration,
            },
        );

        state.updated_unix_seconds = unix_seconds()?;
        write_json(state_path, state)?;
        rendered_segments.push(output_path);
    }
    Ok(rendered_segments)
}

#[allow(clippy::too_many_arguments)]
fn capture_clip<P>(
    request: &RenderRequest,
    clip: &RenderClip,
    index: usize,
    movie_dir: &Path,
    clip_dir: &Path,
    base_path: &Path,
    cancellation: &CancellationToken,
    child: &mut Child,
    control: &mut VConsoleSession,
    mut on_tick: P,
) -> Result<CaptureDiagnostics, RenderError>
where
    P: FnMut(f32, f32),
{
    let ticks_per_second = request.timeline.replay.playback_ticks as f32
        / request.timeline.replay.playback_time_seconds;
    let last_tick = request.timeline.replay.playback_ticks.max(0) as u32;
    let start_tick = source_to_tick(clip.source_start_seconds, clip, ticks_per_second, last_tick);
    let end_tick = source_to_tick(clip.source_end_seconds, clip, ticks_per_second, last_tick);
    if end_tick <= start_tick {
        return Err(RenderError::InvalidPlan(format!(
            "{} 的回放 tick 区间无效",
            clip.clip_id
        )));
    }
    let requested_preroll_ticks = (CAPTURE_PREROLL_SECONDS * ticks_per_second).round() as u32;
    let capture_start_tick = start_tick.saturating_sub(requested_preroll_ticks);
    let preroll_seconds = (start_tick - capture_start_tick) as f32 / ticks_per_second;

    send_commands(control, &["demo_resume".to_string()])?;
    thread::sleep(Duration::from_millis(350));
    send_commands(
        control,
        &[format!("demo_goto {capture_start_tick} absolute pause")],
    )?;
    let observed_tick = wait_for_seek(capture_start_tick, cancellation, child, control)?;

    let camera_commands = camera_commands_for_clip(&request.timeline, clip);
    let mut setup_commands = vec!["demo_timescale 1.000".to_string()];
    setup_commands.extend(hud_commands(request.settings.clean_hud));
    setup_commands.extend(camera_commands.iter().cloned());
    send_commands(control, &setup_commands)?;
    wait_with_cancel(
        Duration::from_millis(250),
        cancellation,
        child,
        control,
        |_| {},
    )?;
    let camera_report =
        execute_logged_commands(control, &["dota_camera_get_lookatpos".to_string()])?;
    let observed_look_at = camera_report
        .console_excerpt
        .iter()
        .find_map(|line| parse_camera_look_at(line))
        .ok_or_else(|| {
            RenderError::ReplayControl(
                "镜头位置反馈不可识别，已停止录制；请保留控制日志以校准当前客户端。".to_string(),
            )
        })?;
    // A real position response proves only that the camera answered. It does
    // not prove the requested hero/mode is active. Never upgrade this to a
    // target-verified cache without a separately validated feedback contract.
    wait_for_seek(capture_start_tick, cancellation, child, control)?;
    let mut diagnostics = CaptureDiagnostics {
        clip_id: clip.clip_id.clone(),
        requested_camera: clip.camera_mode.clone(),
        requested_hero: clip.view_hero.clone(),
        requested_slot: hero_slot_for_clip(&request.timeline, clip),
        requested_tick: capture_start_tick,
        observed_tick,
        observed_look_at,
        camera_target_verified: false,
        verification_status: "camera_target_unverified".to_string(),
        expected_frames: 0,
        captured_frames: 0,
    };
    write_json(&clip_dir.join("capture-diagnostics.json"), &diagnostics)?;

    let prefix = format!(
        "d2h_{}_{}_{}",
        index + 1,
        clip.clip_id.replace('-', "_"),
        unique_stamp()
    );
    let mut capture_files = CaptureFiles::new(movie_dir, &prefix)?;
    let source_duration = clip.source_end_seconds - clip.source_start_seconds;
    let impact_start =
        (clip.source_peak_seconds - clip.source_start_seconds - 1.5).clamp(0.0, source_duration);
    let impact_end = (clip.source_peak_seconds - clip.source_start_seconds + 2.0)
        .clamp(impact_start, source_duration);
    let slow_speed = if request.settings.slow_motion {
        0.72
    } else {
        1.0
    };
    let total_output_duration =
        impact_start + (impact_end - impact_start) / slow_speed + (source_duration - impact_end);
    let output_frame_count = frame_count_for_duration(total_output_duration);
    let preroll_frame_count = frame_count_for_duration(preroll_seconds);
    let capture_frame_count = preroll_frame_count + output_frame_count;
    diagnostics.expected_frames = capture_frame_count;
    write_json(&clip_dir.join("capture-diagnostics.json"), &diagnostics)?;
    let mut diagnostic_record = CaptureDiagnosticRecord {
        path: clip_dir.join("capture-diagnostics.json"),
        diagnostics: &mut diagnostics,
    };
    let impact_start_frame = preroll_frame_count + frame_count_for_duration(impact_start);
    let impact_output_end_frame = preroll_frame_count
        + frame_count_for_duration(impact_start + (impact_end - impact_start) / slow_speed);

    send_commands(
        control,
        &[
            format!(
                "startmovie {prefix} jpg wav jpeg_quality {NATIVE_JPEG_QUALITY} framerate {FRAME_RATE}"
            ),
            format!("demo_pauseatservertick {end_tick}"),
            "demo_resume".to_string(),
        ],
    )?;
    let started = Instant::now();
    // Apply the current clip's target again after playback has resumed. Keep
    // this inside the existing discarded preroll; no source bounds or final
    // duration change, and never substitute another camera mode or hero.
    if let Some(target_frames) =
        camera_reassert_frame_target(&clip.camera_mode, preroll_frame_count, capture_frame_count)
    {
        if target_frames > 0 {
            wait_for_capture_frame_count(
                &mut capture_files,
                target_frames,
                capture_frame_count,
                started,
                &mut diagnostic_record,
                cancellation,
                child,
                control,
                &mut on_tick,
            )?;
        }
        send_commands(control, &camera_commands)?;
    }
    if slow_speed < 1.0 && impact_end > impact_start {
        wait_for_capture_frame_count(
            &mut capture_files,
            impact_start_frame,
            capture_frame_count,
            started,
            &mut diagnostic_record,
            cancellation,
            child,
            control,
            &mut on_tick,
        )?;
        let impact_commands = vec![format!("demo_timescale {slow_speed:.3}")];
        send_commands(control, &impact_commands)?;
        wait_for_capture_frame_count(
            &mut capture_files,
            impact_output_end_frame,
            capture_frame_count,
            started,
            &mut diagnostic_record,
            cancellation,
            child,
            control,
            &mut on_tick,
        )?;
        let result_commands = vec!["demo_timescale 1.000".to_string()];
        send_commands(control, &result_commands)?;
    }
    wait_for_capture_frame_count(
        &mut capture_files,
        capture_frame_count,
        capture_frame_count,
        started,
        &mut diagnostic_record,
        cancellation,
        child,
        control,
        &mut on_tick,
    )?;
    send_commands(control, &["endmovie".to_string(), "demo_pause".to_string()])?;

    let raw_dir = clip_dir.join("raw");
    fs::create_dir_all(&raw_dir)?;
    let captured = wait_for_movie_files(&mut capture_files, cancellation)?;
    let captured_frame_count = captured.iter().filter(|path| is_jpeg(path)).count();
    diagnostics.expected_frames = capture_frame_count;
    diagnostics.captured_frames = captured_frame_count;
    write_json(&clip_dir.join("capture-diagnostics.json"), &diagnostics)?;
    if captured_frame_count < capture_frame_count {
        return Err(RenderError::MissingFrames(clip.clip_id.clone()));
    }
    let (frames, wav) =
        copy_capture_files(&captured, &raw_dir, preroll_frame_count, output_frame_count)?;
    let frame_duration = frames.len() as f32 / FRAME_RATE as f32;
    let wav_duration = probe_media(&request.ffprobe_exe, &wav)
        .map(|media| (media.duration_seconds - preroll_seconds).max(0.0))
        .unwrap_or(frame_duration);
    let encode_duration = total_output_duration
        .min(frame_duration)
        .min(wav_duration)
        .max(1.0);
    if total_output_duration - encode_duration > 2.0 / FRAME_RATE as f32 {
        return Err(RenderError::Media(format!(
            "{} 原生导出时长不足：需要 {:.3} 秒，只得到 {:.3} 秒",
            clip.clip_id, total_output_duration, encode_duration
        )));
    }
    encode_frame_sequence(
        &request.ffmpeg_exe,
        &raw_dir,
        &wav,
        base_path,
        encode_duration,
        preroll_seconds,
    )?;

    for path in captured {
        let _ = fs::remove_file(path);
    }
    Ok(diagnostics)
}

fn apply_replay_emphasis(
    ffmpeg: &Path,
    ffprobe: &Path,
    input: &Path,
    output: &Path,
    clip: &RenderClip,
    enabled: bool,
) -> Result<(), RenderError> {
    if !enabled {
        cache_storage::link_or_copy_new(input, output)?;
        return Ok(());
    }
    let media = probe_media(ffprobe, input)?;
    let source_duration = clip.source_end_seconds - clip.source_start_seconds;
    let source_peak_offset =
        (clip.source_peak_seconds - clip.source_start_seconds).clamp(0.0, source_duration);
    let output_peak = if source_peak_offset <= source_duration {
        source_peak_offset.min(media.duration_seconds)
    } else {
        media.duration_seconds * 0.65
    };
    let replay_start = (output_peak - 1.15).max(0.0);
    let replay_end = (output_peak + 1.0).min(media.duration_seconds);
    if replay_end - replay_start < 0.8 {
        cache_storage::link_or_copy_new(input, output)?;
        return Ok(());
    }
    let filter = format!(
        "[0:v]split=2[vm][vr];\
         [vr]trim=start={replay_start:.3}:end={replay_end:.3},setpts=(PTS-STARTPTS)/0.82[vs];\
         [vm][vs]concat=n=2:v=1:a=0[vout];\
         [0:a]asplit=2[am][ar];\
         [ar]atrim=start={replay_start:.3}:end={replay_end:.3},asetpts=PTS-STARTPTS,atempo=0.82[as];\
         [am][as]concat=n=2:v=0:a=1[aout]"
    );
    let args = vec![
        "-y".to_string(),
        "-i".to_string(),
        path_arg(input),
        "-filter_complex".to_string(),
        filter,
        "-map".to_string(),
        "[vout]".to_string(),
        "-map".to_string(),
        "[aout]".to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-preset".to_string(),
        "medium".to_string(),
        "-crf".to_string(),
        VIDEO_CRF.to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        path_arg(output),
    ];
    run_media_command(ffmpeg, &args, "追加爆点慢放回看")
}

fn encode_frame_sequence(
    ffmpeg: &Path,
    raw_dir: &Path,
    wav: &Path,
    output: &Path,
    duration: f32,
    audio_start_seconds: f32,
) -> Result<(), RenderError> {
    let pattern = raw_dir.join("frame-%06d.jpg");
    let args = vec![
        "-y".to_string(),
        "-framerate".to_string(),
        FRAME_RATE.to_string(),
        "-start_number".to_string(),
        "1".to_string(),
        "-i".to_string(),
        path_arg(&pattern),
        "-ss".to_string(),
        format!("{audio_start_seconds:.3}"),
        "-i".to_string(),
        path_arg(wav),
        "-t".to_string(),
        format!("{duration:.3}"),
        "-vf".to_string(),
        NATIVE_VIDEO_FILTER.to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-preset".to_string(),
        "medium".to_string(),
        "-crf".to_string(),
        VIDEO_CRF.to_string(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        path_arg(output),
    ];
    run_media_command(ffmpeg, &args, "编码 Dota 2 原生帧序列")
}

fn concat_segments(
    ffmpeg: &Path,
    segments: &[PathBuf],
    work_dir: &Path,
    output: &Path,
) -> Result<(), RenderError> {
    let list_path = work_dir.join("concat.txt");
    let mut list = String::new();
    for segment in segments {
        let normalized = segment
            .display()
            .to_string()
            .replace('\\', "/")
            .replace('\'', "'\\''");
        list.push_str(&format!("file '{normalized}'\n"));
    }
    fs::write(&list_path, list)?;
    let args = vec![
        "-y".to_string(),
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-i".to_string(),
        path_arg(&list_path),
        "-c".to_string(),
        "copy".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        path_arg(output),
    ];
    run_media_command(ffmpeg, &args, "组合高光片段")
}

#[allow(clippy::too_many_arguments)]
fn mix_final_audio(
    ffmpeg: &Path,
    video: &Path,
    bgm: Option<&Path>,
    narration: Option<&Path>,
    output: &Path,
    duration: f32,
    settings: &RenderSettings,
) -> Result<(), RenderError> {
    let mut args = vec!["-y".to_string(), "-i".to_string(), path_arg(video)];
    if let Some(path) = bgm {
        args.extend([
            "-stream_loop".to_string(),
            "-1".to_string(),
            "-i".to_string(),
            path_arg(path),
        ]);
    }
    if let Some(path) = narration {
        args.extend(["-i".to_string(), path_arg(path)]);
    }

    let game_volume = settings.game_audio_volume.clamp(0.0, 1.5);
    let bgm_volume = settings.bgm_volume.clamp(0.0, 1.0);
    let fade_out_start = (duration - 1.0).max(0.0);
    let (filter, audio_map) = match (bgm.is_some(), narration.is_some()) {
        (true, true) => (
            format!(
                "[0:a]volume={game_volume:.3}[game];\
                 [1:a]atrim=duration={duration:.3},volume={bgm_volume:.3},afade=t=in:st=0:d=0.5,afade=t=out:st={fade_out_start:.3}:d=1[music];\
                 [music][2:a]sidechaincompress=threshold=0.025:ratio=8:attack=20:release=350[ducked];\
                 [game][ducked][2:a]amix=inputs=3:duration=first:normalize=0,loudnorm=I=-14:TP=-1.0:LRA=11[aout]"
            ),
            "[aout]",
        ),
        (true, false) => (
            format!(
                "[0:a]volume={game_volume:.3}[game];\
                 [1:a]atrim=duration={duration:.3},volume={bgm_volume:.3},afade=t=in:st=0:d=0.5,afade=t=out:st={fade_out_start:.3}:d=1[music];\
                 [game][music]amix=inputs=2:duration=first:normalize=0,loudnorm=I=-14:TP=-1.0:LRA=11[aout]"
            ),
            "[aout]",
        ),
        (false, true) => (
            format!(
                "[0:a]volume={game_volume:.3}[game];\
                 [game][1:a]amix=inputs=2:duration=first:normalize=0,loudnorm=I=-14:TP=-1.0:LRA=11[aout]"
            ),
            "[aout]",
        ),
        (false, false) => (
            format!("[0:a]volume={game_volume:.3},loudnorm=I=-14:TP=-1.0:LRA=11[aout]"),
            "[aout]",
        ),
    };
    args.extend([
        "-filter_complex".to_string(),
        filter,
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        audio_map.to_string(),
        "-c:v".to_string(),
        "copy".to_string(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "-t".to_string(),
        format!("{duration:.3}"),
        "-movflags".to_string(),
        "+faststart".to_string(),
        path_arg(output),
    ]);
    run_media_command(ffmpeg, &args, "混合最终声音")
}

fn run_qc(ffmpeg: &Path, ffprobe: &Path, output: &Path) -> Result<QcReport, RenderError> {
    let media = probe_media(ffprobe, output)?;
    validate_media_contract(&media)?;
    let qc_output = output_with_loader_retry(|| {
        let mut command = background_command(ffmpeg);
        command.args([
            "-hide_banner",
            "-xerror",
            "-i",
            &path_arg(output),
            "-vf",
            "blackdetect=d=0.5:pix_th=0.10,freezedetect=n=-55dB:d=1.5",
            "-af",
            "volumedetect",
            "-f",
            "null",
            "NUL",
        ]);
        command
    })
    .map_err(|error| RenderError::Media(format!("无法运行质量检查：{error}")))?;
    parse_qc_report(output, media, qc_output)
}

fn parse_qc_report(
    output: &Path,
    media: MediaInfo,
    qc_output: Output,
) -> Result<QcReport, RenderError> {
    validate_media_contract(&media)?;
    require_success(&qc_output, "质量检查")?;
    let stderr = String::from_utf8_lossy(&qc_output.stderr);
    let black_events = stderr.matches("black_start:").count();
    let freeze_events = stderr.matches("freeze_start:").count();
    let audio_mean_db = parse_db_value(&stderr, "mean_volume:");
    let audio_peak_db = parse_db_value(&stderr, "max_volume:");
    if audio_mean_db.is_none() || audio_peak_db.is_none() {
        return Err(RenderError::Media(
            "质量检查未返回完整音量检测结果，不能报告已通过。".to_string(),
        ));
    }
    let mut warnings = Vec::new();
    if black_events > 0 {
        warnings.push(format!("检测到 {black_events} 个超过 0.5 秒的黑帧区间。"));
    }
    if freeze_events > 0 {
        warnings.push(format!("检测到 {freeze_events} 个超过 1.5 秒的冻结区间。"));
    }
    Ok(QcReport {
        schema_version: RENDER_SCHEMA_VERSION.to_string(),
        output_path: output.display().to_string(),
        duration_seconds: media.duration_seconds,
        width: media.width,
        height: media.height,
        has_video: media.has_video,
        has_audio: media.has_audio,
        black_events,
        freeze_events,
        audio_mean_db,
        audio_peak_db,
        final_segment_count: 0,
        source_assets_dir: String::new(),
        source_assets: Vec::new(),
        warnings,
    })
}

#[derive(Debug)]
struct MediaInfo {
    duration_seconds: f32,
    width: u32,
    height: u32,
    has_video: bool,
    has_audio: bool,
    video_frames: Option<usize>,
    frame_rate: f32,
}

fn probe_media(ffprobe: &Path, path: &Path) -> Result<MediaInfo, RenderError> {
    let output = output_with_loader_retry(|| {
        let mut command = background_command(ffprobe);
        command.args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,width,height,nb_frames,avg_frame_rate",
            "-of",
            "json",
            &path_arg(path),
        ]);
        command
    })
    .map_err(|error| RenderError::Media(format!("无法运行 FFprobe：{error}")))?;
    if !output.status.success() {
        return Err(RenderError::Media(format!(
            "FFprobe 无法读取 {}：{}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let duration_seconds = value
        .pointer("/format/duration")
        .and_then(|value| value.as_str())
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(0.0);
    let mut width = 0;
    let mut height = 0;
    let mut has_video = false;
    let mut has_audio = false;
    let mut video_frames = None;
    let mut frame_rate = 0.0;
    if let Some(streams) = value.get("streams").and_then(|value| value.as_array()) {
        for stream in streams {
            match stream.get("codec_type").and_then(|value| value.as_str()) {
                Some("video") => {
                    has_video = true;
                    frame_rate = stream
                        .get("avg_frame_rate")
                        .and_then(|value| value.as_str())
                        .and_then(|value| value.split_once('/'))
                        .and_then(|(numerator, denominator)| {
                            let numerator = numerator.parse::<f32>().ok()?;
                            let denominator = denominator.parse::<f32>().ok()?;
                            (denominator > 0.0).then_some(numerator / denominator)
                        })
                        .unwrap_or(0.0);
                    video_frames = stream
                        .get("nb_frames")
                        .and_then(|value| value.as_str())
                        .and_then(|value| value.parse().ok());
                    width = stream
                        .get("width")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as u32;
                    height = stream
                        .get("height")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as u32;
                }
                Some("audio") => has_audio = true,
                _ => {}
            }
        }
    }
    Ok(MediaInfo {
        duration_seconds,
        width,
        height,
        has_video,
        has_audio,
        video_frames,
        frame_rate,
    })
}

fn validate_request(request: &RenderRequest) -> Result<(), RenderError> {
    if request.timeline.source_sha256 != request.source_sha256 {
        return Err(RenderError::InvalidPlan(
            "录像时间轴与原录像身份不一致，请重新分析。".to_string(),
        ));
    }
    if !request.source_replay.is_file() {
        return Err(RenderError::MissingReplay(
            request.source_replay.display().to_string(),
        ));
    }
    if file_sha256(&request.source_replay)? != request.source_sha256 {
        return Err(RenderError::InvalidPlan(
            "原录像内容已变化，与分析数据不一致；请重新分析。".to_string(),
        ));
    }
    if !request.timeline.replay.playback_time_seconds.is_finite()
        || request.timeline.replay.playback_time_seconds <= 0.0
        || request.timeline.replay.playback_ticks <= 0
    {
        return Err(RenderError::InvalidPlan("录像时间轴无效。".to_string()));
    }
    if !request.dota2_exe.is_file() {
        return Err(RenderError::MissingTool("Dota 2".to_string()));
    }
    if request.clips.is_empty() {
        return Err(RenderError::InvalidPlan(
            "请至少选择一个高光片段。".to_string(),
        ));
    }
    let mut ids = HashSet::new();
    let mut take_groups = HashMap::<String, Vec<&RenderClip>>::new();
    for clip in &request.clips {
        if clip.clip_id.is_empty()
            || clip.clip_id.len() > 160
            || !clip
                .clip_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || !clip.source_start_seconds.is_finite()
            || !clip.source_end_seconds.is_finite()
            || !clip.source_peak_seconds.is_finite()
            || !clip.anchor_seconds.is_finite()
        {
            return Err(RenderError::InvalidPlan("片段编号或时间无效。".to_string()));
        }
        if !ids.insert(&clip.clip_id) {
            return Err(RenderError::InvalidPlan(format!(
                "片段 {} 重复出现",
                clip.clip_id
            )));
        }
        if !valid_source_frame_range(clip.source_start_seconds, clip.source_end_seconds) {
            return Err(RenderError::InvalidPlan(format!(
                "{} 的时长必须在 1 到 100 秒之间",
                clip.clip_id
            )));
        }
        if clip.source_peak_seconds < clip.source_start_seconds
            || clip.source_peak_seconds > clip.source_end_seconds
        {
            return Err(RenderError::InvalidPlan(format!(
                "{} 的爆点不在选定时间范围内",
                clip.clip_id
            )));
        }
        if !request.timeline.replay.players.is_empty()
            && clip.view_hero.is_some()
            && hero_slot_for_clip(&request.timeline, clip).is_none()
        {
            return Err(RenderError::InvalidPlan(format!(
                "{} 的所选英雄不在本局十人阵容中",
                clip.clip_id
            )));
        }
        if let Some(group_id) = clip.take_group_id.as_deref() {
            if group_id.is_empty()
                || group_id.len() > 160
                || !group_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(RenderError::InvalidPlan(format!(
                    "{} 的素材场次编号无效",
                    clip.clip_id
                )));
            }
            take_groups
                .entry(group_id.to_string())
                .or_default()
                .push(clip);
        } else if clip.take_role != RenderTakeRole::Primary || !clip.include_in_final {
            return Err(RenderError::InvalidPlan(format!(
                "{} 是独立素材，必须作为默认入片的主机位",
                clip.clip_id
            )));
        }
    }
    if !request.clips.iter().any(|clip| clip.include_in_final) {
        return Err(RenderError::InvalidPlan(
            "请至少保留一个默认入片的主机位素材。".to_string(),
        ));
    }
    for (group_id, group) in take_groups {
        let primaries = group
            .iter()
            .copied()
            .filter(|clip| clip.take_role == RenderTakeRole::Primary)
            .collect::<Vec<_>>();
        if primaries.len() != 1 {
            return Err(RenderError::InvalidPlan(format!(
                "素材场次 {group_id} 必须且只能有一个主机位"
            )));
        }
        let primary = primaries[0];
        if !primary.include_in_final {
            return Err(RenderError::InvalidPlan(format!(
                "素材场次 {group_id} 的主机位必须默认入片"
            )));
        }
        if group
            .iter()
            .any(|clip| clip.take_role == RenderTakeRole::Alternate && clip.include_in_final)
        {
            return Err(RenderError::InvalidPlan(format!(
                "素材场次 {group_id} 的备用机位不能直接增加成片时长"
            )));
        }
        for take in group {
            if take.candidate_id != primary.candidate_id
                || (take.source_start_seconds - primary.source_start_seconds).abs() > 0.001
                || (take.source_end_seconds - primary.source_end_seconds).abs() > 0.001
            {
                return Err(RenderError::InvalidPlan(format!(
                    "素材场次 {group_id} 的所有机位必须对应同一事件和完全相同的时间段"
                )));
            }
        }
    }
    if request.settings.bgm_mode == BgmMode::Custom
        && request
            .settings
            .custom_bgm_path
            .as_deref()
            .map(Path::new)
            .is_none_or(|path| !path.is_file())
    {
        return Err(RenderError::InvalidPlan(
            "请选择一个可读取的自选 BGM 文件。".to_string(),
        ));
    }
    Ok(())
}

fn render_fingerprint(request: &RenderRequest) -> Result<String, RenderError> {
    render_fingerprint_with_pipeline(request, CAPTURE_PIPELINE_VERSION)
}

fn render_fingerprint_with_pipeline(
    request: &RenderRequest,
    capture_pipeline: &str,
) -> Result<String, RenderError> {
    #[derive(Serialize)]
    struct Fingerprint<'a> {
        schema: &'static str,
        capture_pipeline: &'a str,
        source_sha256: &'a str,
        clips: &'a [RenderClip],
        settings: &'a RenderSettings,
    }
    let bytes = serde_json::to_vec(&Fingerprint {
        schema: RENDER_SCHEMA_VERSION,
        capture_pipeline,
        source_sha256: &request.source_sha256,
        clips: &request.clips,
        settings: &request.settings,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn load_or_create_state(path: &Path, fingerprint: &str) -> Result<RenderState, RenderError> {
    if path.is_file() {
        match serde_json::from_slice::<RenderState>(&fs::read(path)?) {
            Ok(state) if state.fingerprint == fingerprint => return Ok(state),
            previous => {
                let backup = path.with_extension(format!("rejected-{}.json", unique_stamp()));
                copy_file(path, &backup)?;
                execution::record(
                    "cache_state_rejected",
                    serde_json::json!({
                        "backup": backup, "parseError": previous.err().map(|error| error.to_string()),
                    }),
                )?;
            }
        }
    }
    let state = RenderState {
        schema_version: RENDER_SCHEMA_VERSION.to_string(),
        fingerprint: fingerprint.to_string(),
        updated_unix_seconds: unix_seconds()?,
        segments: BTreeMap::new(),
    };
    write_json(path, &state)?;
    Ok(state)
}

struct PreparedReplay {
    path: PathBuf,
    replay_reference: String,
    created_by_app: bool,
}

fn prepare_replay(
    source: &Path,
    replay_dir: &Path,
    job_id: &str,
    expected_sha256: &str,
) -> Result<PreparedReplay, RenderError> {
    // Always use a newly-created ASCII alias. In particular, never interpolate
    // the user's original filename into a console command, even in replays/.
    let safe_job: String = job_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(18)
        .collect();
    let stem = format!("d2h_{}_{}", safe_job, unique_stamp());
    let path = replay_dir.join(format!("{stem}.dem"));
    let mut prepared = PreparedReplay {
        path,
        replay_reference: format!("replays/{stem}"),
        created_by_app: false,
    };
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&prepared.path)?;
    prepared.created_by_app = true;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        execution::check_cancelled()?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    output.sync_all()?;
    drop(output);
    if format!("{:x}", hasher.finalize()) != expected_sha256
        || file_sha256(&prepared.path)? != expected_sha256
    {
        return Err(RenderError::InvalidPlan(
            "复制录像期间源内容发生变化或副本校验失败，请重新分析。".to_string(),
        ));
    }
    Ok(prepared)
}

impl Drop for PreparedReplay {
    fn drop(&mut self) {
        if self.created_by_app {
            let _ = fs::remove_file(&self.path);
        }
    }
}

// The socket is retained until this exact child is confirmed exited, including
// cancellation, errors and unwinding. Cleanup never opens another connection.
struct OwnedDota(Child, Option<VConsoleSession>);

impl Drop for OwnedDota {
    fn drop(&mut self) {
        let pid = self.0.id();
        let mut first_attempt = true;
        // Also hold the owning stack during unwinding until the same Child is
        // confirmed stopped. A second failed best-effort attempt is not enough.
        wait_for_verified_cleanup(
            || {
                shutdown_dota(
                    &mut self.0,
                    std::mem::take(&mut first_attempt),
                    self.1.as_mut(),
                )
            },
            |error| {
                let _ = execution::record(
                    "cleanup_pending",
                    serde_json::json!({
                        "pid": pid, "error": error, "duringUnwind": std::thread::panicking(),
                    }),
                );
            },
            || thread::sleep(Duration::from_secs(2)),
        );
    }
}

fn dota_game_dir(dota2_exe: &Path) -> Result<PathBuf, RenderError> {
    dota2_exe
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .ok_or_else(|| RenderError::MissingTool("Dota 2 游戏目录".to_string()))
}

fn launch_dota(dota2_exe: &Path) -> Result<Child, RenderError> {
    Command::new(dota2_exe)
        .args(dota_launch_args())
        .current_dir(dota_game_dir(dota2_exe)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| RenderError::Media(format!("无法启动 Dota 2：{error}")))
}

fn dota_launch_args() -> [&'static str; 4] {
    ["-insecure", "-vconsole", "-console", "-novid"]
}

fn wait_for_vconsole(
    child: &mut Child,
    control: &mut Option<VConsoleSession>,
    cancellation: &CancellationToken,
    mut on_wait: impl FnMut(u64),
) -> Result<(), RenderError> {
    let started = Instant::now();
    let mut attempt = 0;
    let mut last_error = "尚未收到控制连接反馈".to_string();
    while started.elapsed() < Duration::from_secs(90) {
        check_cancelled(cancellation)?;
        if child.try_wait()?.is_some() {
            return Err(RenderError::DotaExited);
        }
        attempt += 1;
        // A refused connection may be retried before the port is ready. Once
        // accepted, preserve the socket and decoder across handshake timeouts.
        let connected = if control.is_none() {
            VConsoleSession::connect(Duration::from_secs(2)).and_then(|session| {
                *control = Some(session);
                execution::record(
                    "vconsole_connected",
                    serde_json::json!({"attempt": attempt, "pid": child.id()}),
                )?;
                Ok(())
            })
        } else {
            Ok(())
        };
        let result = connected.and_then(|_| {
            control
                .as_mut()
                .expect("connected")
                .probe(Duration::from_secs(2))
        });
        match result {
            Ok(report) => {
                execution::record(
                    "vconsole_ready",
                    serde_json::json!({"attempt": attempt, "report": report}),
                )?;
                return Ok(());
            }
            Err(error) => {
                last_error = error.to_string();
                execution::record(
                    "vconsole_probe_failed",
                    serde_json::json!({
                        "attempt": attempt,
                        "elapsedMilliseconds": started.elapsed().as_millis(),
                        "error": last_error,
                    }),
                )?;
                on_wait(started.elapsed().as_secs());
                // Only an unready port or an incomplete handshake can wait.
                // An accepted socket with invalid/I/O feedback fails closed.
                if control.is_some() && !matches!(error, ReplayControlError::MarkerTimeout) {
                    return Err(RenderError::ReplayControl(last_error));
                }
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err(RenderError::VConsoleUnavailable(last_error))
}

// The no-argument demo_goto branch in the installed engine2.dll prints:
// "  Currently playing %d of %d ticks. Minutes:%.2f File:%s".
// Unknown client formats fail closed and are preserved in the diagnostic log.
fn parse_demo_current_tick(line: &str) -> Option<u32> {
    let value = line.trim().strip_prefix("Currently playing ")?;
    let (current, remaining) = value.split_once(" of ")?;
    let (total, _) = remaining.split_once(" ticks.")?;
    let current: u32 = current.parse().ok()?;
    let total: u32 = total.parse().ok()?;
    (current <= total).then_some(current)
}

// Verified string in the installed client.dll. This confirms position only.
fn parse_camera_look_at(line: &str) -> Option<[f32; 3]> {
    let value = line.trim().strip_prefix("Camera look-at position: ")?;
    let mut coordinates = value.split_ascii_whitespace();
    let result = [
        coordinates.next()?.parse().ok()?,
        coordinates.next()?.parse().ok()?,
        coordinates.next()?.parse().ok()?,
    ];
    (coordinates.next().is_none() && result.iter().all(|value: &f32| value.is_finite()))
        .then_some(result)
}

fn seek_observation_is_stable(previous: Option<u32>, current: Option<u32>, target: u32) -> bool {
    current.is_some_and(|tick| tick.abs_diff(target) <= 5 && previous == current)
}

fn wait_for_seek(
    target_tick: u32,
    cancellation: &CancellationToken,
    child: &mut Child,
    control: &mut VConsoleSession,
) -> Result<u32, RenderError> {
    let started = Instant::now();
    let mut previous_tick = None;
    let mut last_tick = None;
    let mut last_error = None;
    while started.elapsed() < Duration::from_secs(15) {
        check_cancelled(cancellation)?;
        if child.try_wait()?.is_some() {
            return Err(RenderError::DotaExited);
        }
        match execute_logged_commands(control, &[DEMO_STATUS_COMMAND.to_string()]) {
            Ok(report) => {
                let current = report
                    .console_excerpt
                    .iter()
                    .rev()
                    .find_map(|line| parse_demo_current_tick(line));
                execution::record(
                    "seek_observation",
                    serde_json::json!({
                        "targetTick": target_tick, "observedTick": current, "previousTick": previous_tick,
                    }),
                )?;
                if seek_observation_is_stable(previous_tick, current, target_tick) {
                    return Ok(current.expect("stable observation has a tick"));
                }
                previous_tick = current;
                last_tick = current.or(last_tick);
            }
            Err(error) => {
                previous_tick = None;
                last_error = Some(error.to_string());
            }
        }
        wait_with_cancel(
            Duration::from_millis(650),
            cancellation,
            child,
            control,
            |_| {},
        )?;
    }
    execution::record(
        "seek_failed",
        serde_json::json!({
            "query": DEMO_STATUS_COMMAND, "targetTick": target_tick,
            "observedTick": last_tick, "controlError": last_error,
        }),
    )?;
    Err(RenderError::ReplayControl(seek_failure_message(
        target_tick,
        last_tick,
        last_error.as_deref(),
    )))
}

fn seek_failure_message(
    target_tick: u32,
    last_tick: Option<u32>,
    last_error: Option<&str>,
) -> String {
    let reason = if last_tick.is_some() {
        "录像尚未稳定到达指定位置，本次导出已停止。"
    } else {
        "未能读取录像当前播放位置，本次导出已停止。"
    };
    let observed = last_tick.map_or_else(|| "未收到".to_string(), |tick| format!("{tick} tick"));
    let mut message = format!(
        "回放跳转未确认：{reason}\n目标位置：{target_tick} tick；最近收到的位置：{observed}。\n请保留本次控制日志以便排查。"
    );
    if let Some(error) = last_error {
        message.push_str(&format!("\n控制反馈：{error}"));
    }
    message
}

fn execute_logged_commands(
    control: &mut VConsoleSession,
    commands: &[String],
) -> Result<VConsoleExecutionReport, RenderError> {
    let result = control.execute(commands, VCONSOLE_COMMAND_TIMEOUT);
    execution::record(
        "vconsole_commands",
        serde_json::json!({
            "commands": commands,
            "response": result.as_ref().ok(),
            "error": result.as_ref().err().map(ToString::to_string),
        }),
    )?;
    result.map_err(|error| RenderError::ReplayControl(error.to_string()))
}

fn send_commands(control: &mut VConsoleSession, commands: &[String]) -> Result<(), RenderError> {
    execute_logged_commands(control, commands).map(|_| ())
}

fn wait_for_verified_cleanup(
    mut attempt: impl FnMut() -> Option<String>,
    mut pending: impl FnMut(&str),
    mut pause: impl FnMut(),
) {
    // Cancellation requests stop production, never ownership accounting. This
    // function intentionally has no cancellation/timeout escape: callers may
    // release their activity gate only after the owned process is confirmed out.
    let mut failures = 0_u64;
    while let Some(error) = attempt() {
        failures = failures.saturating_add(1);
        // Bound repeated diagnostics even if Windows cannot confirm this handle
        // for hours. Ownership checks continue after these eight notices.
        if failures <= 128 && failures.is_power_of_two() {
            pending(&error);
        }
        pause();
    }
}

fn finish_dota_session<F>(
    child: &mut Child,
    control: &mut Option<VConsoleSession>,
    total_clips: usize,
    on_progress: &mut F,
) where
    F: FnMut(RenderProgress),
{
    let pid = child.id();
    let mut first_attempt = true;
    wait_for_verified_cleanup(
        || shutdown_dota(child, std::mem::take(&mut first_attempt), control.as_mut()),
        |error| {
            // A full disk or broken progress receiver must not bypass cleanup.
            let _ = execution::record(
                "cleanup_pending",
                serde_json::json!({"pid": pid, "error": error}),
            );
            emit(
                on_progress,
                "cleanup_pending",
                "running",
                &format!(
                    "Dota 2（本任务 PID {pid}）尚未确认关闭，正在重试收尾。取消、关闭或更新将等待收尾完成。{error}"
                ),
                80,
                total_clips,
                total_clips,
            );
        },
        || thread::sleep(Duration::from_secs(2)),
    );
    let _ = execution::record(
        "dota_stopped",
        serde_json::json!({"pid": pid, "confirmed": true}),
    );
}

fn shutdown_dota(
    child: &mut Child,
    restore_session: bool,
    control: Option<&mut VConsoleSession>,
) -> Option<String> {
    match child.try_wait() {
        Ok(Some(_)) => return None,
        Ok(None) => {}
        Err(error) => {
            return Some(format!(
                "无法核对本任务的 Dota 2 句柄，等待确认后再操作：{error}"
            ));
        }
    }
    if restore_session {
        if let Some(control) = control {
            let _ = control.execute(
                &[
                    "endmovie".to_string(),
                    "demo_pause".to_string(),
                    "cl_drawhud 1".to_string(),
                    "dota_hide_cursor 0".to_string(),
                ],
                Duration::from_secs(2),
            );
        }
        let pid = child.id().to_string();
        #[cfg(windows)]
        {
            let mut command = background_command("taskkill");
            command.args(["/PID", &pid, "/T"]);
            let _ = execution::cleanup_output(command);
        }
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(8) {
            match child.try_wait() {
                Ok(Some(_)) => return None,
                Ok(None) => thread::sleep(Duration::from_millis(250)),
                Err(error) => return Some(format!("关闭 Dota 2 状态检查失败：{error}")),
            }
        }
    }
    match child.kill().and_then(|_| child.wait()) {
        Ok(_) => None,
        Err(error) => Some(format!("Dota 2 自动关闭失败，请手动关闭：{error}")),
    }
}

fn dota_is_running() -> Result<bool, RenderError> {
    #[cfg(windows)]
    {
        windows_process_is_running("dota2.exe")
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

fn validate_recording_clients(dota_running: bool, steam_running: bool) -> Result<(), RenderError> {
    if dota_running {
        return Err(RenderError::DotaAlreadyRunning);
    }
    if !steam_running {
        return Err(RenderError::SteamRequiredForRecording);
    }
    Ok(())
}

fn steam_is_running() -> Result<bool, RenderError> {
    #[cfg(windows)]
    {
        windows_process_is_running("steam.exe")
    }
    #[cfg(not(windows))]
    {
        Ok(true)
    }
}

#[cfg(windows)]
fn windows_process_is_running(image_name: &str) -> Result<bool, RenderError> {
    let mut command = background_command("tasklist");
    command.args([
        "/FI",
        &format!("IMAGENAME eq {image_name}"),
        "/FO",
        "CSV",
        "/NH",
    ]);
    let output = execution::output(command)?;
    require_success(&output, "进程状态检查")?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .to_ascii_lowercase()
        .contains(&image_name.to_ascii_lowercase()))
}

fn wait_for_movie_files(
    capture_files: &mut CaptureFiles,
    cancellation: &CancellationToken,
) -> Result<Vec<PathBuf>, RenderError> {
    let started = Instant::now();
    let mut previous_files = BTreeMap::new();
    let mut stable_since = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        check_cancelled(cancellation)?;
        let captured = capture_files.collect()?;
        if captured != previous_files {
            previous_files = captured;
            stable_since = Instant::now();
        } else if captured
            .iter()
            .filter(|(path, size)| is_jpeg(path) && **size > 0)
            .count()
            >= FRAME_RATE as usize
            && stable_since.elapsed() >= Duration::from_secs(2)
        {
            return Ok(captured.into_keys().collect());
        }
        thread::sleep(Duration::from_millis(400));
    }
    Ok(Vec::new())
}

fn copy_capture_files(
    captured: &[PathBuf],
    raw_dir: &Path,
    frame_skip: usize,
    frame_limit: usize,
) -> Result<(Vec<PathBuf>, PathBuf), RenderError> {
    let mut images = captured
        .iter()
        .filter(|path| is_jpeg(path))
        .cloned()
        .collect::<Vec<_>>();
    images.sort();
    let images = images
        .into_iter()
        .skip(frame_skip)
        .take(frame_limit)
        .collect::<Vec<_>>();
    let wav = captured
        .iter()
        .find(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
        })
        .ok_or_else(|| RenderError::Media("Dota 2 原生导出没有生成 WAV 音轨。".to_string()))?;
    let mut copied = Vec::with_capacity(images.len());
    for (index, source) in images.iter().enumerate() {
        let destination = raw_dir.join(format!("frame-{:06}.jpg", index + 1));
        copy_file(source, &destination)?;
        copied.push(destination);
    }
    let wav_destination = raw_dir.join("game.wav");
    copy_file(wav, &wav_destination)?;
    Ok((copied, wav_destination))
}

fn camera_commands_for_clip(timeline: &TimelineDocument, clip: &RenderClip) -> Vec<String> {
    let slot = hero_slot_for_clip(timeline, clip);
    match clip.camera_mode {
        ClipCameraMode::Directed => vec!["dota_spectator_mode 0".to_string()],
        ClipCameraMode::FreeCamera => {
            let mut commands = vec!["dota_spectator_mode 1".to_string()];
            if let Some(slot) = slot {
                commands.push(format!("dota_spectator_hero_index {slot}"));
                commands.push(format!("dota_camera_focus_player {slot}"));
            }
            commands
        }
        ClipCameraMode::HeroChase => {
            let mut commands = Vec::new();
            if let Some(slot) = slot {
                commands.push(format!("dota_spectator_hero_index {slot}"));
                commands.push(format!("dota_camera_focus_player {slot}"));
            }
            // Focusing is a positioning operation; finish by selecting the
            // requested follow mode so it remains the last camera action.
            commands.push("dota_spectator_mode 2".to_string());
            commands
        }
        ClipCameraMode::PlayerPerspective => {
            let mut commands = Vec::new();
            if let Some(slot) = slot {
                commands.push(format!("dota_spectator_hero_index {slot}"));
                commands.push(format!("dota_camera_focus_player {slot}"));
            }
            commands.push("dota_spectator_mode 3".to_string());
            commands
        }
    }
}

fn camera_reassert_frame_target(
    camera_mode: &ClipCameraMode,
    preroll_frame_count: usize,
    capture_frame_count: usize,
) -> Option<usize> {
    matches!(
        camera_mode,
        ClipCameraMode::PlayerPerspective | ClipCameraMode::HeroChase
    )
    .then(|| {
        CAMERA_REASSERT_CAPTURE_FRAMES
            .min(preroll_frame_count.saturating_sub(1))
            .min(capture_frame_count)
    })
}

fn hud_commands(clean_hud: bool) -> Vec<String> {
    let mut commands = vec!["dota_spectator_options_enabled 0".to_string()];
    if clean_hud {
        commands.push("dota_spectator_hudhide".to_string());
        commands.extend(clean_hud_commands());
    }
    commands
}

fn clean_hud_commands() -> Vec<String> {
    [
        "sv_cheats 1",
        "dota_hud_hide_mainhud 1",
        "dota_hud_hide_topbar 1",
        "dota_hud_hide_minimap 1",
        "dota_hud_hide_overlaymap 1",
        "dota_show_itempickups 0",
        "r_draw_selected_ring 0",
        "cl_drawhud 0",
        "dota_hide_cursor 1",
        "r_drawpanorama 0",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn hero_slot_for_clip(timeline: &TimelineDocument, clip: &RenderClip) -> Option<u8> {
    let hero = clip.view_hero.as_deref()?;
    timeline
        .replay
        .players
        .iter()
        .find(|player| player.hero_name == hero)
        .map(|player| player.slot)
}

fn source_to_tick(seconds: f32, clip: &RenderClip, ticks_per_second: f32, last_tick: u32) -> u32 {
    (clip.anchor_tick as f32 + (seconds - clip.anchor_seconds) * ticks_per_second)
        .round()
        .clamp(0.0, last_tick as f32) as u32
}

fn frame_count_for_duration(duration_seconds: f32) -> usize {
    (duration_seconds.max(0.0) * FRAME_RATE as f32)
        .round()
        .max(0.0) as usize
}

struct CaptureDiagnosticRecord<'a> {
    path: PathBuf,
    diagnostics: &'a mut CaptureDiagnostics,
}

/// Observations only: none of these clocks alter the capture deadline.
struct CaptureWaitTelemetry {
    target_frames: usize,
    total_frames: usize,
    captured_frames: usize,
    wait_started_elapsed: Duration,
    last_sample_elapsed: Duration,
    last_frame_increase_elapsed: Duration,
    timed_out: bool,
}

impl CaptureWaitTelemetry {
    fn new(
        target_frames: usize,
        total_frames: usize,
        captured_frames: usize,
        elapsed: Duration,
    ) -> Self {
        Self {
            target_frames,
            total_frames,
            captured_frames,
            wait_started_elapsed: elapsed,
            last_sample_elapsed: elapsed,
            last_frame_increase_elapsed: elapsed,
            timed_out: false,
        }
    }

    fn observe(&mut self, captured_frames: usize, elapsed: Duration) {
        if captured_frames > self.captured_frames {
            self.last_frame_increase_elapsed = elapsed;
        }
        self.captured_frames = captured_frames;
    }

    fn sample_due(&mut self, elapsed: Duration) -> bool {
        if elapsed.saturating_sub(self.last_sample_elapsed) < CAPTURE_PROGRESS_LOG_INTERVAL {
            return false;
        }
        self.last_sample_elapsed = elapsed;
        true
    }

    fn snapshot(&self, clip_id: &str, elapsed: Duration, timeout: Duration) -> serde_json::Value {
        serde_json::json!({
            "clipId": clip_id,
            "targetFrames": self.target_frames,
            "totalFrames": self.total_frames,
            "capturedFrames": self.captured_frames,
            "elapsedMilliseconds": elapsed.as_millis(),
            "waitStartedElapsedMilliseconds": self.wait_started_elapsed.as_millis(),
            "waitElapsedMilliseconds": elapsed.saturating_sub(self.wait_started_elapsed).as_millis(),
            "lastFrameIncreaseElapsedMilliseconds": self.last_frame_increase_elapsed.as_millis(),
            "stalledMilliseconds": elapsed.saturating_sub(self.last_frame_increase_elapsed).as_millis(),
            "timeoutSeconds": timeout.as_secs_f32(),
        })
    }
}

fn finish_capture_wait(
    telemetry: &CaptureWaitTelemetry,
    diagnostic_record: &mut CaptureDiagnosticRecord<'_>,
    elapsed: Duration,
    timeout: Duration,
    result: Result<(), RenderError>,
) -> Result<(), RenderError> {
    diagnostic_record.diagnostics.captured_frames = telemetry.captured_frames;
    let mut details = telemetry.snapshot(&diagnostic_record.diagnostics.clip_id, elapsed, timeout);
    details["status"] = serde_json::json!(match &result {
        Ok(()) => "complete",
        Err(RenderError::Cancelled) => "cancelled",
        Err(_) => "failed",
    });
    details["error"] = serde_json::json!(result.as_ref().err().map(ToString::to_string));
    let log_result = execution::record(
        if telemetry.timed_out {
            "capture_timeout"
        } else {
            "capture_wait_finished"
        },
        details,
    );
    let diagnostic_result = write_json(&diagnostic_record.path, diagnostic_record.diagnostics);
    // Keep the primary capture error even if its final diagnostic writes fail.
    // On a successful capture, retain the existing fail-closed logging policy.
    result?;
    log_result?;
    diagnostic_result
}

#[allow(clippy::too_many_arguments)]
fn wait_for_capture_frame_count<P>(
    capture_files: &mut CaptureFiles,
    target_frames: usize,
    total_frames: usize,
    started: Instant,
    diagnostic_record: &mut CaptureDiagnosticRecord<'_>,
    cancellation: &CancellationToken,
    child: &mut Child,
    control: &mut VConsoleSession,
    on_tick: &mut P,
) -> Result<(), RenderError>
where
    P: FnMut(f32, f32),
{
    let total_seconds = total_frames as f32 / FRAME_RATE as f32;
    let timeout_seconds = (total_seconds * 6.0 + 30.0).clamp(45.0, 900.0);
    let timeout = Duration::from_secs_f32(timeout_seconds);
    let mut telemetry = CaptureWaitTelemetry::new(
        target_frames,
        total_frames,
        diagnostic_record.diagnostics.captured_frames,
        started.elapsed(),
    );
    execution::record(
        "capture_wait_start",
        telemetry.snapshot(
            &diagnostic_record.diagnostics.clip_id,
            started.elapsed(),
            timeout,
        ),
    )?;
    let result = (|| {
        while started.elapsed() < timeout {
            check_cancelled(cancellation)?;
            if child.try_wait()?.is_some() {
                return Err(RenderError::DotaExited);
            }
            control.drain_pending()?;
            let captured_frames = capture_files
                .collect()?
                .iter()
                .filter(|(path, size)| is_jpeg(path) && **size > 0)
                .count();
            let elapsed = started.elapsed();
            telemetry.observe(captured_frames, elapsed);
            if telemetry.sample_due(elapsed) {
                execution::record(
                    "capture_progress",
                    telemetry.snapshot(&diagnostic_record.diagnostics.clip_id, elapsed, timeout),
                )?;
            }
            let captured_seconds = captured_frames.min(total_frames) as f32 / FRAME_RATE as f32;
            on_tick(captured_seconds, total_seconds);
            if captured_frames >= target_frames {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(250));
        }
        telemetry.timed_out = true;
        Err(RenderError::Media(format!(
            "Dota 2 原生逐帧导出超时：需要 {target_frames} 帧，只得到 {} 帧",
            telemetry.captured_frames
        )))
    })();
    finish_capture_wait(
        &telemetry,
        diagnostic_record,
        started.elapsed(),
        timeout,
        result,
    )
}

fn is_jpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"))
}

fn wait_with_cancel<P>(
    duration: Duration,
    cancellation: &CancellationToken,
    child: &mut Child,
    control: &mut VConsoleSession,
    mut on_tick: P,
) -> Result<(), RenderError>
where
    P: FnMut(f32),
{
    let started = Instant::now();
    while started.elapsed() < duration {
        check_cancelled(cancellation)?;
        if child.try_wait()?.is_some() {
            return Err(RenderError::DotaExited);
        }
        control.drain_pending()?;
        on_tick(started.elapsed().as_secs_f32());
        thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), RenderError> {
    if cancellation.is_cancelled() {
        Err(RenderError::Cancelled)
    } else {
        Ok(())
    }
}

fn check_command(path: &Path, args: &[&str], label: &str) -> Result<(), RenderError> {
    if !path.is_file() && path.components().count() > 1 {
        return Err(RenderError::MissingTool(label.to_string()));
    }
    let output = output_with_loader_retry(|| {
        let mut command = background_command(path);
        command.args(args);
        command
    });
    match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(RenderError::Media(format!(
            "{label} 启动检查失败：{}",
            command_failure_text(&output, 8)
        ))),
        Err(error) => Err(RenderError::Media(format!("{label} 无法启动：{error}"))),
    }
}

fn run_media_command(executable: &Path, args: &[String], purpose: &str) -> Result<(), RenderError> {
    let output = output_with_loader_retry(|| {
        let mut command = background_command(executable);
        command.args(args);
        command
    })
    .map_err(|error| RenderError::Media(format!("{purpose}无法启动：{error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RenderError::Media(format!(
            "{purpose}失败：{}",
            command_failure_text(&output, 12)
        )))
    }
}

fn command_failure_text(output: &Output, lines: usize) -> String {
    let stderr = tail_text(output, lines);
    let exit_code = output
        .status
        .code()
        .map(|code| format!("0x{:08X}", code as u32))
        .unwrap_or_else(|| "未知".to_string());
    if stderr.is_empty() {
        format!("进程退出码 {exit_code}")
    } else {
        format!("进程退出码 {exit_code}；{stderr}")
    }
}

fn tail_text(output: &Output, lines: usize) -> String {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .rev()
        .take(lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ")
}

fn segment_cache_is_valid(
    request: &RenderRequest,
    clip: &RenderClip,
    state: &SegmentState,
) -> bool {
    let expected = expected_clip_duration(clip, &request.settings);
    if !cache_metadata_matches(state, clip, expected) {
        return false;
    }
    let path = Path::new(&state.output_path);
    path.is_file()
        && fs::metadata(path).is_ok_and(|metadata| metadata.len() > 1024)
        && file_sha256(path).is_ok_and(|sha| sha == state.content_sha256)
        && probe_media(&request.ffprobe_exe, path)
            .is_ok_and(|media| validate_segment_media(&media, expected).is_ok())
}

fn cache_metadata_matches(state: &SegmentState, clip: &RenderClip, expected: f32) -> bool {
    matches!(
        state.status.as_str(),
        "complete" | "camera_target_unverified"
    ) && state.quality_checked
        && state.capture_pipeline == CAPTURE_PIPELINE_VERSION
        && state.candidate_id == clip.candidate_id
        && state.content_sha256.len() == 64
        && (state.expected_duration_seconds - expected).abs() <= 0.001
        && (state.duration_seconds - expected).abs() <= 0.1
}

fn expected_clip_duration(clip: &RenderClip, settings: &RenderSettings) -> f32 {
    let source = clip.source_end_seconds - clip.source_start_seconds;
    let impact_start =
        (clip.source_peak_seconds - clip.source_start_seconds - 1.5).clamp(0.0, source);
    let impact_end =
        (clip.source_peak_seconds - clip.source_start_seconds + 2.0).clamp(impact_start, source);
    let speed = if settings.slow_motion { 0.72 } else { 1.0 };
    let duration = impact_start + (impact_end - impact_start) / speed + source - impact_end;
    if settings.replay_emphasis {
        let peak = (clip.source_peak_seconds - clip.source_start_seconds)
            .clamp(0.0, source)
            .min(duration);
        let replay = (peak + 1.0).min(duration) - (peak - 1.15).max(0.0);
        duration + if replay >= 0.8 { replay / 0.82 } else { 0.0 }
    } else {
        duration
    }
}

fn validate_media_contract(media: &MediaInfo) -> Result<(), RenderError> {
    if !media.has_video
        || !media.has_audio
        || media.width == 0
        || media.height == 0
        || !media.width.is_multiple_of(2)
        || !media.height.is_multiple_of(2)
        || !media.duration_seconds.is_finite()
        || media.duration_seconds < 1.0
    {
        return Err(RenderError::Media(
            "媒体缺少有效视频/音轨，或分辨率、时长不符合成片要求。".to_string(),
        ));
    }
    Ok(())
}

fn valid_source_frame_range(start: f32, end: f32) -> bool {
    start.is_finite()
        && end.is_finite()
        && start >= 0.0
        && end > start
        && (30..=3000).contains(&((end * 30.0).round() as i64 - (start * 30.0).round() as i64))
}

fn validate_segment_media(media: &MediaInfo, expected: f32) -> Result<(), RenderError> {
    validate_media_contract(media)?;
    let frames = frame_count_for_duration(expected);
    if (media.frame_rate - FRAME_RATE as f32).abs() > 0.01
        || (media.duration_seconds - expected).abs() > 0.1
        || media
            .video_frames
            .is_none_or(|actual| actual.abs_diff(frames) > 2)
    {
        return Err(RenderError::Media(format!(
            "片段时长或帧数与方案不一致：预期 {expected:.3} 秒/{frames} 帧，实际 {:.3} 秒/{:?} 帧。",
            media.duration_seconds, media.video_frames
        )));
    }
    Ok(())
}

fn require_success(output: &Output, purpose: &str) -> Result<(), RenderError> {
    if output.status.success() {
        Ok(())
    } else {
        Err(RenderError::Media(format!(
            "{purpose}失败：{}",
            command_failure_text(output, 12)
        )))
    }
}

fn file_sha256(path: &Path) -> Result<String, RenderError> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = Sha256::new();
    loop {
        execution::check_cancelled()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn copy_file(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> io::Result<u64> {
    let mut input = fs::File::open(source)?;
    let mut output = fs::File::create(destination)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut length = 0;
    loop {
        execution::check_cancelled()?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        length += count as u64;
    }
    Ok(length)
}

fn unique_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn impact_cues(ffprobe_exe: &Path, segments: &[(RenderClip, PathBuf)]) -> Vec<f32> {
    let mut cues = Vec::new();
    let mut cursor = 0.0;
    for (clip, segment) in segments {
        let duration = probe_media(ffprobe_exe, segment)
            .map(|media| media.duration_seconds)
            .unwrap_or_else(|_| clip.source_end_seconds - clip.source_start_seconds);
        let peak_offset =
            (clip.source_peak_seconds - clip.source_start_seconds).clamp(0.0, duration);
        cues.push(cursor + peak_offset);
        cursor += duration;
    }
    cues
}

fn write_system_narration(path: &Path, text: &str) -> Result<(), String> {
    let script = r#"
Add-Type -AssemblyName System.Speech
$speaker = New-Object System.Speech.Synthesis.SpeechSynthesizer
$speaker.Rate = 1
$speaker.Volume = 92
$speaker.SetOutputToWaveFile($env:D2H_NARRATION_PATH)
$speaker.Speak($env:D2H_NARRATION_TEXT)
$speaker.Dispose()
"#;
    let mut command = background_command("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("D2H_NARRATION_PATH", path)
        .env("D2H_NARRATION_TEXT", text);
    let output = execution::output(command).map_err(|error| error.to_string())?;
    if output.status.success() && path.is_file() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn parse_db_value(text: &str, marker: &str) -> Option<f32> {
    text.lines().find_map(|line| {
        let (_, value) = line.split_once(marker)?;
        value
            .trim()
            .split_ascii_whitespace()
            .next()?
            .parse::<f32>()
            .ok()
    })
}

fn clip_percent(completed: usize, total: usize) -> u8 {
    if total == 0 {
        10
    } else {
        (10.0 + completed as f32 / total as f32 * 68.0).round() as u8
    }
}

fn emit<F>(
    on_progress: &mut F,
    stage: &str,
    status: &str,
    message: &str,
    percent: u8,
    current_clip: usize,
    total_clips: usize,
) where
    F: FnMut(RenderProgress),
{
    on_progress(RenderProgress {
        stage: stage.to_string(),
        status: status.to_string(),
        message: message.to_string(),
        percent,
        current_clip,
        total_clips,
    });
}

fn path_arg(path: &Path) -> String {
    path.as_os_str().to_string_lossy().into_owned()
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), RenderError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| json_io_error("创建记录目录", parent, error))?;
    }
    let temporary = path.with_extension(format!("pending-{}.json", unique_stamp()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| json_io_error("创建临时记录", &temporary, error))?;
    file.write_all(&serde_json::to_vec_pretty(value)?)
        .map_err(|error| json_io_error("写入临时记录", &temporary, error))?;
    file.sync_all()
        .map_err(|error| json_io_error("刷新临时记录", &temporary, error))?;
    drop(file);
    atomic_replace(&temporary, path).map_err(|error| json_io_error("替换正式记录", path, error))?;
    Ok(())
}

fn json_io_error(operation: &str, path: &Path, error: io::Error) -> RenderError {
    RenderError::Io(io::Error::new(
        error.kind(),
        format!(
            "保存录制记录失败；操作：{operation}；路径：{}；原因：{error}",
            path.display()
        ),
    ))
}

fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        // Rust file writes support extended Windows paths, but a direct Win32
        // rename also needs those absolute names. The new destination may not
        // exist yet, so resolve its existing parent rather than the file.
        let source_path = fs::canonicalize(source)?;
        let destination_absolute = std::path::absolute(destination)?;
        let parent = destination_absolute
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "记录路径没有父目录"))?;
        let name = destination_absolute
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "记录路径没有文件名"))?;
        let destination_path = fs::canonicalize(parent)?.join(name);
        let source: Vec<u16> = source_path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let destination: Vec<u16> = destination_path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // Both names are app-owned files in the same directory. No recursive
        // operations or user media are involved in replacing this JSON state.
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, destination)
    }
}

fn unix_seconds() -> Result<u64, RenderError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| RenderError::Clock)
}

impl From<ReplayControlError> for RenderError {
    fn from(error: ReplayControlError) -> Self {
        Self::ReplayControl(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BgmMode, CameraStyle, ClipCameraMode, RenderClip, RenderRequest, RenderSettings,
        RenderTakeRole, available_output_stem, camera_commands_for_clip, concat_segments,
        encode_frame_sequence, export_source_assets, frame_count_for_duration, hero_slot_for_clip,
        hud_commands, mix_final_audio, parse_db_value, probe_media, replay_output_stem, run_qc,
        select_final_segments, source_asset_numbering, source_to_tick,
    };
    use d2_highlights_core::{ParserIdentity, ReplayMetadata, ReplayPlayer, TimelineDocument};

    #[test]
    fn cleanup_retries_and_cannot_report_completion_before_exit_is_confirmed() {
        use std::cell::{Cell, RefCell};
        let attempts = Cell::new(0);
        let pauses = Cell::new(0);
        let pending = RefCell::new(Vec::new());
        super::wait_for_verified_cleanup(
            || {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                match attempt {
                    1 => Some("fixture: termination denied".to_string()),
                    2 => Some("fixture: process still running".to_string()),
                    3 => None,
                    _ => panic!("must stop retrying after verified exit"),
                }
            },
            |reason| {
                assert!(attempts.get() < 3);
                pending.borrow_mut().push(reason.to_string());
            },
            || pauses.set(pauses.get() + 1),
        );
        // The caller may release its activity permit only at this point.
        assert_eq!(attempts.get(), 3);
        assert_eq!(pending.borrow().len(), 2);
        assert_eq!(pauses.get(), 2);
    }

    #[test]
    fn cancellation_during_cleanup_is_reported_only_after_verified_exit() {
        use std::cell::Cell;
        let temp = tempfile::tempdir().unwrap();
        let cancellation = super::CancellationToken::new();
        let _scope =
            super::execution::Scope::enter(cancellation.clone(), temp.path().join("cleanup.jsonl"))
                .unwrap();
        let attempts = Cell::new(0);
        super::wait_for_verified_cleanup(
            || {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                if attempt == 1 {
                    Some("fixture: cleanup pending".to_string())
                } else {
                    assert!(cancellation.is_cancelled());
                    None
                }
            },
            |_| cancellation.cancel(),
            || {},
        );
        assert_eq!(attempts.get(), 2);
        assert!(matches!(
            super::check_cancelled(&cancellation),
            Err(super::RenderError::Cancelled)
        ));
    }

    #[test]
    fn already_confirmed_cleanup_does_not_emit_pending_or_retry() {
        super::wait_for_verified_cleanup(
            || None,
            |_| panic!("already closed must not emit cleanup_pending"),
            || panic!("already closed must not retry"),
        );
    }

    #[test]
    fn cleanup_diagnostics_are_bounded_while_ownership_checks_continue() {
        use std::cell::Cell;
        let attempts = Cell::new(0);
        let notices = Cell::new(0);
        let pauses = Cell::new(0);
        super::wait_for_verified_cleanup(
            || {
                attempts.set(attempts.get() + 1);
                (attempts.get() <= 20_000).then(|| "fixture: still pending".to_string())
            },
            |_| notices.set(notices.get() + 1),
            || pauses.set(pauses.get() + 1),
        );
        assert_eq!(attempts.get(), 20_001);
        assert_eq!(pauses.get(), 20_000);
        assert_eq!(notices.get(), 8);
    }

    #[test]
    fn seek_only_accepts_the_current_field_of_the_native_status_format() {
        assert_eq!(
            super::parse_demo_current_tick(
                "  Currently playing 9000 of 30000 ticks. Minutes:5.00 File:replays/d2h_fixture"
            ),
            Some(9000)
        );
        for line in [
            "DemoFileInfo: playback_time: 1000",
            "playback_ticks: 30000",
            "Current tick: 9000; total ticks: 30000",
            "total ticks: 30000",
            "Currently playing 40000 of 30000 ticks. Minutes:0 File:a",
            "echo Currently playing 1000 of 3000 ticks. Minutes:0 File:a",
            "Currently playing x of 3000 ticks.",
        ] {
            assert_eq!(super::parse_demo_current_tick(line), None, "{line}");
        }
        assert!(!super::seek_observation_is_stable(
            Some(9000),
            Some(9000),
            1000
        ));
        assert!(!super::seek_observation_is_stable(None, Some(1000), 1000));
        assert!(!super::seek_observation_is_stable(
            Some(999),
            Some(1000),
            1000
        ));
        assert!(!super::seek_observation_is_stable(Some(1000), None, 1000));
        assert!(super::seek_observation_is_stable(
            Some(1000),
            Some(1000),
            1000
        ));
    }

    #[test]
    fn seek_failure_distinguishes_missing_feedback_from_wrong_position() {
        let missing = super::seek_failure_message(9000, None, None);
        assert!(missing.contains("未能读取录像当前播放位置"));
        assert!(missing.contains("最近收到的位置：未收到"));
        assert!(!missing.contains("None"));
        let wrong = super::seek_failure_message(9000, Some(500), Some("fixture: disconnected"));
        assert!(wrong.contains("录像尚未稳定到达指定位置"));
        assert!(wrong.contains("最近收到的位置：500 tick"));
        assert!(wrong.contains("控制反馈：fixture: disconnected"));
    }

    #[test]
    fn camera_feedback_is_position_only_and_rejects_invalid_values() {
        assert_eq!(
            super::parse_camera_look_at(
                "Camera look-at position: 123.500000 -456.250000 128.000000"
            ),
            Some([123.5, -456.25, 128.0])
        );
        for line in [
            "Camera look-at position: NaN 0 0",
            "Camera look-at position: 1 2",
            "Camera look-at position: 1 2 3 4",
            "dota_spectator_mode 3",
        ] {
            assert!(super::parse_camera_look_at(line).is_none());
        }
    }

    #[test]
    fn editing_boundaries_are_measured_in_source_frames() {
        for frame in [0, 3331, 40001, 93272] {
            let start = frame as f32 / 30.0;
            assert!(super::valid_source_frame_range(
                start,
                (frame + 30) as f32 / 30.0
            ));
            assert!(super::valid_source_frame_range(
                start,
                (frame + 3000) as f32 / 30.0
            ));
            assert!(!super::valid_source_frame_range(
                start,
                (frame + 29) as f32 / 30.0
            ));
            assert!(!super::valid_source_frame_range(
                start,
                (frame + 3001) as f32 / 30.0
            ));
        }
        assert!(!super::valid_source_frame_range(f32::NAN, 10.0));
        assert!(!super::valid_source_frame_range(-1.0, 10.0));
    }

    fn fixture_media(duration: f32) -> super::MediaInfo {
        super::MediaInfo {
            duration_seconds: duration,
            width: 1920,
            height: 1080,
            has_video: true,
            has_audio: true,
            video_frames: Some((duration * super::FRAME_RATE as f32).round() as usize),
            frame_rate: super::FRAME_RATE as f32,
        }
    }

    fn fixture_output(code: i32, stderr: &str) -> std::process::Output {
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code as u32)
        };
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code << 8)
        };
        std::process::Output {
            status,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn failed_or_incomplete_qc_never_returns_zero_defect_success() {
        let path = std::path::Path::new("fixture.mp4");
        assert!(
            super::parse_qc_report(
                path,
                fixture_media(12.25),
                fixture_output(
                    1,
                    "No such filter: 'freezedetect'\nError initializing filters!\n"
                )
            )
            .is_err()
        );
        assert!(super::parse_qc_report(path, fixture_media(12.25), fixture_output(0, "")).is_err());
        let mut missing_audio = fixture_media(12.25);
        missing_audio.has_audio = false;
        assert!(
            super::parse_qc_report(
                path,
                missing_audio,
                fixture_output(0, "mean_volume: -14.2 dB\nmax_volume: -0.8 dB")
            )
            .is_err()
        );
        let report = super::parse_qc_report(
            path,
            fixture_media(12.25),
            fixture_output(0, "mean_volume: -14.2 dB\nmax_volume: -0.8 dB"),
        )
        .unwrap();
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn resume_cache_requires_media_completion_pipeline_identity_and_expected_duration() {
        let clip = mirana_clip(ClipCameraMode::PlayerPerspective);
        let mut state = super::SegmentState {
            candidate_id: clip.candidate_id.clone(),
            output_path: "fixture.mp4".to_string(),
            duration_seconds: 4.0,
            expected_duration_seconds: 4.0,
            status: "complete".to_string(),
            capture_pipeline: super::CAPTURE_PIPELINE_VERSION.to_string(),
            content_sha256: "a".repeat(64),
            quality_checked: true,
            camera_target_verified: false,
        };
        assert!(super::cache_metadata_matches(&state, &clip, 4.0));
        state.status = "camera_target_unverified".into();
        assert!(super::cache_metadata_matches(&state, &clip, 4.0));
        state.quality_checked = false;
        assert!(!super::cache_metadata_matches(&state, &clip, 4.0));
        state.quality_checked = true;
        state.status = "capturing".into();
        assert!(!super::cache_metadata_matches(&state, &clip, 4.0));
        state.status = "complete".into();
        state.duration_seconds = 2.0;
        assert!(!super::cache_metadata_matches(&state, &clip, 4.0));
        state.duration_seconds = 4.0;
        state.capture_pipeline = "old".to_string();
        assert!(!super::cache_metadata_matches(&state, &clip, 4.0));
        assert!(super::validate_segment_media(&fixture_media(2.0), 7.7).is_err());
        assert!(super::validate_segment_media(&fixture_media(7.7), 7.7).is_ok());
        let mut wrong_frames = fixture_media(7.7);
        wrong_frames.video_frames = Some(30);
        assert!(super::validate_segment_media(&wrong_frames, 7.7).is_err());
    }

    #[test]
    fn replay_alias_is_safe_even_when_original_is_already_in_replays() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["my match.dem", "中文 录像.dem", "a;quit.dem"] {
            let source = temp.path().join(name);
            std::fs::write(&source, b"original fixture").unwrap();
            let hash = super::file_sha256(&source).unwrap();
            let prepared = super::prepare_replay(&source, temp.path(), "d2h-test", &hash).unwrap();
            let alias = prepared.path.clone();
            assert_ne!(alias, source);
            assert_eq!(std::fs::read(&alias).unwrap(), b"original fixture");
            assert!(
                d2_highlights_replay_control::validate_command(&format!(
                    "playdemo {}",
                    prepared.replay_reference
                ))
                .is_ok()
            );
            drop(prepared);
            assert!(!alias.exists());
            assert_eq!(std::fs::read(&source).unwrap(), b"original fixture");
        }
    }

    #[test]
    fn replay_replacement_between_inspection_and_copy_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("fixture.dem");
        std::fs::write(&source, b"old data").unwrap();
        let hash = super::file_sha256(&source).unwrap();
        std::fs::write(&source, b"new data").unwrap();
        assert!(super::prepare_replay(&source, temp.path(), "d2h-test", &hash).is_err());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
        assert_eq!(std::fs::read(source).unwrap(), b"new data");
    }

    #[cfg(windows)]
    #[test]
    fn windows_json_atomic_replace_supports_long_chinese_paths() {
        use std::os::windows::ffi::OsStrExt;
        let temp = tempfile::tempdir().unwrap();
        let mut directory = temp.path().to_path_buf();
        while directory.as_os_str().encode_wide().count() < 280 {
            directory = directory.join("剧情诊断_长路径兼容回归");
        }
        let path = directory.join("capture-diagnostics.json");
        super::write_json(&path, &serde_json::json!({"frames": 0})).unwrap();
        super::write_json(&path, &serde_json::json!({"frames": 1080})).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["frames"], 1080);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    }

    #[test]
    fn json_write_failure_reports_operation_and_exact_path() {
        let temp = tempfile::tempdir().unwrap();
        let parent_file = temp.path().join("不是文件夹");
        std::fs::write(&parent_file, b"keep original").unwrap();
        let error = super::write_json(
            &parent_file.join("state.json"),
            &serde_json::json!({"frames": 0}),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("创建记录目录"));
        assert!(error.contains(&parent_file.display().to_string()));
        assert_eq!(std::fs::read(parent_file).unwrap(), b"keep original");
    }

    #[test]
    fn corrupt_cache_state_is_preserved_and_rebuilt_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        std::fs::write(&path, b"{truncated").unwrap();
        let rebuilt = super::load_or_create_state(&path, "fixture").unwrap();
        assert_eq!(rebuilt.fingerprint, "fixture");
        super::write_json(&path, &rebuilt).unwrap();
        assert_eq!(
            super::load_or_create_state(&path, "fixture")
                .unwrap()
                .fingerprint,
            "fixture"
        );
        let backup = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().contains("rejected-"))
            .unwrap();
        assert_eq!(std::fs::read(backup.path()).unwrap(), b"{truncated");
    }

    #[cfg(windows)]
    #[test]
    fn only_windows_dll_initialization_failure_is_retried() {
        assert!(super::is_dll_init_failure(Some(0xC000_0142_u32 as i32)));
        assert!(!super::is_dll_init_failure(Some(1)));
        assert!(!super::is_dll_init_failure(None));
    }

    #[cfg(windows)]
    #[test]
    fn windows_helper_process_has_no_console_or_error_dialog_mode() {
        let script = r#"
Add-Type -Namespace D2H -Name NativeConsole -MemberDefinition '[DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();'
if ([D2H.NativeConsole]::GetConsoleWindow() -eq [IntPtr]::Zero) { exit 0 } else { exit 1 }
"#;
        let output = super::background_command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "helper acquired a console window: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let expected = super::SEM_FAILCRITICALERRORS
            | super::SEM_NOGPFAULTERRORBOX
            | super::SEM_NOOPENFILEERRORBOX;
        let actual = unsafe { windows_sys::Win32::System::Diagnostics::Debug::GetErrorMode() };
        assert_eq!(actual & expected, expected);
    }

    #[test]
    fn manual_seconds_map_relative_to_candidate_anchor() {
        let clip = RenderClip {
            clip_id: "clip-003".to_string(),
            candidate_id: "hl-003".to_string(),
            view_hero: None,
            camera_mode: ClipCameraMode::Directed,
            take_group_id: None,
            take_role: RenderTakeRole::Primary,
            include_in_final: true,
            source_start_seconds: 2073.8335,
            source_peak_seconds: 2093.8335,
            source_end_seconds: 2101.8335,
            anchor_seconds: 2093.8335,
            anchor_tick: 64_455,
        };

        assert_eq!(
            source_to_tick(clip.source_start_seconds, &clip, 30.0, 100_000),
            63_855
        );
        assert_eq!(
            source_to_tick(clip.source_end_seconds, &clip, 30.0, 100_000),
            64_695
        );
    }

    #[test]
    fn output_names_follow_the_replay_and_never_replace_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let replay = std::path::Path::new("fixtures")
            .join("replays")
            .join("1234567890.dem");
        let stem = replay_output_stem(&replay);
        assert_eq!(stem, "1234567890");
        assert_eq!(available_output_stem(temp.path(), &stem), "1234567890");

        std::fs::write(temp.path().join("1234567890.mp4"), b"old").unwrap();
        assert_eq!(available_output_stem(temp.path(), &stem), "1234567890_2");

        std::fs::write(temp.path().join("1234567890_2_质量报告.json"), b"{}").unwrap();
        assert_eq!(available_output_stem(temp.path(), &stem), "1234567890_3");
    }

    #[test]
    fn native_capture_waits_for_the_requested_output_frame_count() {
        assert_eq!(frame_count_for_duration(0.0), 0);
        assert_eq!(frame_count_for_duration(1.0 / 30.0), 2);
        assert_eq!(frame_count_for_duration(8.0), 480);
    }

    #[test]
    fn capture_progress_samples_are_throttled_without_losing_stall_observations() {
        use std::time::Duration;
        let mut telemetry = super::CaptureWaitTelemetry::new(2044, 2044, 0, Duration::ZERO);
        telemetry.observe(10, Duration::from_millis(250));
        assert!(!telemetry.sample_due(Duration::from_millis(4999)));
        assert!(telemetry.sample_due(Duration::from_secs(5)));
        assert!(!telemetry.sample_due(Duration::from_millis(5001)));
        telemetry.observe(10, Duration::from_secs(9));
        assert!(telemetry.sample_due(Duration::from_secs(10)));
        assert!(!telemetry.sample_due(Duration::from_millis(10_001)));
        let snapshot = telemetry.snapshot(
            "clip-fixture",
            Duration::from_secs(10),
            Duration::from_millis(234_400),
        );
        assert_eq!(snapshot["capturedFrames"], 10);
        assert_eq!(snapshot["lastFrameIncreaseElapsedMilliseconds"], 250);
        assert_eq!(snapshot["stalledMilliseconds"], 9750);
    }

    #[test]
    fn capture_wait_phases_keep_the_shared_capture_clock_and_previous_frame_count() {
        use std::time::Duration;
        let mut telemetry =
            super::CaptureWaitTelemetry::new(2044, 2044, 30, Duration::from_secs(70));
        telemetry.observe(40, Duration::from_secs(71));
        telemetry.observe(40, Duration::from_secs(75));
        let snapshot = telemetry.snapshot(
            "clip-fixture",
            Duration::from_secs(78),
            Duration::from_millis(234_400),
        );
        assert_eq!(snapshot["elapsedMilliseconds"], 78_000);
        assert_eq!(snapshot["waitStartedElapsedMilliseconds"], 70_000);
        assert_eq!(snapshot["waitElapsedMilliseconds"], 8000);
        assert_eq!(snapshot["lastFrameIncreaseElapsedMilliseconds"], 71_000);
        assert_eq!(snapshot["stalledMilliseconds"], 7000);
        assert_eq!(snapshot["targetFrames"], 2044);
        assert_eq!(snapshot["totalFrames"], 2044);
    }

    fn fixture_capture_diagnostics() -> super::CaptureDiagnostics {
        super::CaptureDiagnostics {
            clip_id: "clip-fixture".into(),
            requested_camera: ClipCameraMode::PlayerPerspective,
            requested_hero: Some("npc_dota_hero_mirana".into()),
            requested_slot: Some(0),
            requested_tick: 9000,
            observed_tick: 9000,
            observed_look_at: [0.0; 3],
            camera_target_verified: false,
            verification_status: "camera_target_unverified".into(),
            expected_frames: 2044,
            captured_frames: 0,
        }
    }

    #[test]
    fn capture_timeout_retains_the_last_count_in_both_diagnostic_records() {
        use std::time::Duration;
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("render.jsonl");
        let _scope =
            super::execution::Scope::enter(super::CancellationToken::new(), log_path.clone())
                .unwrap();
        let mut diagnostics = fixture_capture_diagnostics();
        let diagnostic_path = temp.path().join("capture-diagnostics.json");
        let mut record = super::CaptureDiagnosticRecord {
            path: diagnostic_path.clone(),
            diagnostics: &mut diagnostics,
        };
        let mut telemetry = super::CaptureWaitTelemetry::new(2044, 2044, 0, Duration::ZERO);
        telemetry.observe(1651, Duration::from_secs(233));
        telemetry.timed_out = true;
        let primary_error = "fixture capture timeout";
        let result = super::finish_capture_wait(
            &telemetry,
            &mut record,
            Duration::from_millis(234_400),
            Duration::from_millis(234_400),
            Err(super::RenderError::Media(primary_error.into())),
        );
        assert!(result.unwrap_err().to_string().contains(primary_error));
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(diagnostic_path).unwrap()).unwrap();
        assert_eq!(saved["expected_frames"], 2044);
        assert_eq!(saved["captured_frames"], 1651);
        let event: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(log_path).unwrap().trim()).unwrap();
        assert_eq!(event["event"], "capture_timeout");
        assert_eq!(event["details"]["capturedFrames"], 1651);
        assert_eq!(event["details"]["targetFrames"], 2044);
        assert_eq!(event["details"]["stalledMilliseconds"], 1400);
        assert_eq!(event["details"]["status"], "failed");
        assert!(
            event["details"]["error"]
                .as_str()
                .unwrap()
                .contains(primary_error)
        );
    }

    #[test]
    fn capture_cancellation_logs_the_last_count_without_replacing_its_error() {
        use std::time::Duration;
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("render.jsonl");
        let _scope =
            super::execution::Scope::enter(super::CancellationToken::new(), log_path.clone())
                .unwrap();
        let mut diagnostics = fixture_capture_diagnostics();
        let mut record = super::CaptureDiagnosticRecord {
            path: temp.path().join("capture-diagnostics.json"),
            diagnostics: &mut diagnostics,
        };
        let mut telemetry = super::CaptureWaitTelemetry::new(2044, 2044, 0, Duration::ZERO);
        telemetry.observe(25, Duration::from_millis(500));
        let result = super::finish_capture_wait(
            &telemetry,
            &mut record,
            Duration::from_millis(750),
            Duration::from_millis(234_400),
            Err(super::RenderError::Cancelled),
        );
        assert!(matches!(result, Err(super::RenderError::Cancelled)));
        assert_eq!(diagnostics.captured_frames, 25);
        let event: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(log_path).unwrap().trim()).unwrap();
        assert_eq!(event["event"], "capture_wait_finished");
        assert_eq!(event["details"]["status"], "cancelled");
    }

    #[test]
    fn capture_primary_failure_survives_failed_final_log_and_diagnostic_writes() {
        use std::time::Duration;
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("render.jsonl");
        let _scope =
            super::execution::Scope::enter(super::CancellationToken::new(), log_path.clone())
                .unwrap();
        std::fs::remove_file(&log_path).unwrap();
        std::fs::create_dir(&log_path).unwrap();
        let parent_file = temp.path().join("not-a-directory");
        std::fs::write(&parent_file, b"keep").unwrap();
        let mut diagnostics = fixture_capture_diagnostics();
        let mut record = super::CaptureDiagnosticRecord {
            path: parent_file.join("capture-diagnostics.json"),
            diagnostics: &mut diagnostics,
        };
        let telemetry = super::CaptureWaitTelemetry::new(2044, 2044, 1651, Duration::ZERO);
        let result = super::finish_capture_wait(
            &telemetry,
            &mut record,
            Duration::from_secs(1),
            Duration::from_millis(234_400),
            Err(super::RenderError::DotaExited),
        );
        assert!(matches!(result, Err(super::RenderError::DotaExited)));
        let successful_capture = super::finish_capture_wait(
            &telemetry,
            &mut record,
            Duration::from_secs(1),
            Duration::from_millis(234_400),
            Ok(()),
        );
        assert!(matches!(successful_capture, Err(super::RenderError::Io(_))));
        assert_eq!(std::fs::read(parent_file).unwrap(), b"keep");
    }

    #[test]
    fn native_resolution_and_capture_cadence_survive_validation() {
        let mut media = fixture_media(2.0);
        media.width = 3840;
        media.height = 2160;
        assert!(super::validate_segment_media(&media, 2.0).is_ok());
        media.frame_rate = 30.0;
        assert!(super::validate_segment_media(&media, 2.0).is_err());
        media.frame_rate = super::FRAME_RATE as f32;
        media.width = 0;
        assert!(super::validate_media_contract(&media).is_err());
    }

    #[test]
    fn render_settings_json_matches_desktop_contract() {
        let settings = RenderSettings {
            camera_style: CameraStyle::AutoDirector,
            bgm_mode: BgmMode::GameOnly,
            ..RenderSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();

        assert!(json.contains("\"cameraStyle\":\"auto_director\""));
        assert!(json.contains("\"bgmMode\":\"game_only\""));
        assert!(json.contains("\"cleanHud\":true"));
    }

    #[test]
    fn synchronized_takes_share_a_scene_number_and_primary_is_a() {
        let mut alternate = mirana_clip(ClipCameraMode::HeroChase);
        alternate.clip_id = "clip-close".to_string();
        alternate.take_group_id = Some("story-001".to_string());
        alternate.take_role = RenderTakeRole::Alternate;
        alternate.include_in_final = false;
        let mut primary = mirana_clip(ClipCameraMode::PlayerPerspective);
        primary.clip_id = "clip-player".to_string();
        primary.take_group_id = Some("story-001".to_string());
        let standalone = mirana_clip(ClipCameraMode::PlayerPerspective);

        let numbering = source_asset_numbering(&[alternate, primary, standalone]);

        assert_eq!(numbering[0], ("S001-B".to_string(), 1, 2));
        assert_eq!(numbering[1], ("S001-A".to_string(), 1, 1));
        assert_eq!(numbering[2], ("S002-A".to_string(), 2, 1));
    }

    #[test]
    fn alternate_take_is_exported_but_not_joined_into_default_cut() {
        let mut primary = mirana_clip(ClipCameraMode::PlayerPerspective);
        primary.take_group_id = Some("story-001".to_string());
        let mut alternate = mirana_clip(ClipCameraMode::HeroChase);
        alternate.clip_id = "clip-close".to_string();
        alternate.take_group_id = Some("story-001".to_string());
        alternate.take_role = RenderTakeRole::Alternate;
        alternate.include_in_final = false;
        let segments = vec!["player.mp4".into(), "close.mp4".into()];

        let selected = select_final_segments(&[primary, alternate], &segments).unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0.clip_id, "clip-001");
        assert_eq!(selected[0].1, std::path::PathBuf::from("player.mp4"));
    }

    #[test]
    fn source_assets_are_written_with_matching_scene_take_names() {
        let temp = tempfile::tempdir().unwrap();
        let player_segment = temp.path().join("player.mp4");
        let close_segment = temp.path().join("close.mp4");
        std::fs::write(&player_segment, b"player").unwrap();
        std::fs::write(&close_segment, b"close").unwrap();
        let mut primary = mirana_clip(ClipCameraMode::PlayerPerspective);
        primary.take_group_id = Some("story-001".to_string());
        let mut alternate = mirana_clip(ClipCameraMode::HeroChase);
        alternate.clip_id = "clip-close".to_string();
        alternate.take_group_id = Some("story-001".to_string());
        alternate.take_role = RenderTakeRole::Alternate;
        alternate.include_in_final = false;
        let request = RenderRequest {
            job_id: "d2h-test".to_string(),
            source_sha256: "abc".to_string(),
            job_dir: temp.path().to_path_buf(),
            source_replay: temp.path().join("fixture.dem"),
            dota2_exe: temp.path().join("dota2.exe"),
            ffmpeg_exe: temp.path().join("ffmpeg.exe"),
            ffprobe_exe: temp.path().join("ffprobe.exe"),
            timeline: mirana_timeline(),
            clips: vec![primary, alternate],
            settings: RenderSettings::default(),
            resume_fingerprint: None,
        };
        let output_dir = temp.path().join("source-assets");

        let assets =
            export_source_assets(&request, &[player_segment, close_segment], &output_dir).unwrap();

        assert_eq!(assets[0].asset_id, "S001-A");
        assert_eq!(assets[1].asset_id, "S001-B");
        assert!(output_dir.join("S001-A_玩家视角.mp4").is_file());
        assert!(output_dir.join("S001-B_英雄跟随.mp4").is_file());
        assert!(output_dir.join("素材清单.json").is_file());
    }

    #[test]
    fn legacy_render_clip_defaults_to_a_primary_final_take() {
        let json = r#"{
            "clipId":"clip-legacy",
            "candidateId":"hl-001",
            "viewHero":null,
            "cameraMode":"player_perspective",
            "sourceStartSeconds":10.0,
            "sourcePeakSeconds":12.0,
            "sourceEndSeconds":14.0,
            "anchorSeconds":12.0,
            "anchorTick":360
        }"#;
        let clip: RenderClip = serde_json::from_str(json).unwrap();

        assert_eq!(clip.take_role, RenderTakeRole::Primary);
        assert!(clip.include_in_final);
        assert!(clip.take_group_id.is_none());
    }

    #[test]
    fn selected_hero_maps_to_the_demo_spectator_slot() {
        let timeline = mirana_timeline();
        let clip = mirana_clip(ClipCameraMode::PlayerPerspective);

        assert_eq!(hero_slot_for_clip(&timeline, &clip), Some(7));
    }

    #[test]
    fn player_view_and_hero_chase_use_the_selected_player_slot() {
        let timeline = mirana_timeline();
        let player_view =
            camera_commands_for_clip(&timeline, &mirana_clip(ClipCameraMode::PlayerPerspective));
        let hero_chase =
            camera_commands_for_clip(&timeline, &mirana_clip(ClipCameraMode::HeroChase));

        assert_eq!(
            player_view,
            vec![
                "dota_spectator_hero_index 7",
                "dota_camera_focus_player 7",
                "dota_spectator_mode 3",
            ]
        );
        assert_eq!(
            hero_chase,
            vec![
                "dota_spectator_hero_index 7",
                "dota_camera_focus_player 7",
                "dota_spectator_mode 2",
            ]
        );
    }

    #[test]
    fn mixed_story_targets_are_selected_per_clip_including_repeated_heroes() {
        let mut timeline = mirana_timeline();
        let player_template = timeline.replay.players[0].clone();
        timeline.replay.players = [
            (2, "npc_dota_hero_morphling"),
            (0, "npc_dota_hero_ogre_magi"),
            (1, "npc_dota_hero_ember_spirit"),
        ]
        .into_iter()
        .map(|(slot, hero)| {
            let mut player = player_template.clone();
            player.slot = slot;
            player.hero_name = hero.to_string();
            player
        })
        .collect();

        for (hero, slot, mode, mode_command) in [
            (
                "npc_dota_hero_ember_spirit",
                1,
                ClipCameraMode::PlayerPerspective,
                "dota_spectator_mode 3",
            ),
            (
                "npc_dota_hero_ember_spirit",
                1,
                ClipCameraMode::PlayerPerspective,
                "dota_spectator_mode 3",
            ),
            (
                "npc_dota_hero_morphling",
                2,
                ClipCameraMode::PlayerPerspective,
                "dota_spectator_mode 3",
            ),
            (
                "npc_dota_hero_ogre_magi",
                0,
                ClipCameraMode::HeroChase,
                "dota_spectator_mode 2",
            ),
            (
                "npc_dota_hero_morphling",
                2,
                ClipCameraMode::PlayerPerspective,
                "dota_spectator_mode 3",
            ),
        ] {
            let mut clip = mirana_clip(mode);
            clip.view_hero = Some(hero.to_string());
            let commands = camera_commands_for_clip(&timeline, &clip);
            assert_eq!(
                commands,
                vec![
                    format!("dota_spectator_hero_index {slot}"),
                    format!("dota_camera_focus_player {slot}"),
                    mode_command.to_string(),
                ],
                "the clip must select its own hero even when the prior clip chose the same hero"
            );
            for command in &commands {
                assert!(d2_highlights_replay_control::build_cmnd_packet(command).is_ok());
            }
        }
    }

    #[test]
    fn every_player_slot_is_selected_independently_of_roster_order() {
        let mut timeline = mirana_timeline();
        let player_template = timeline.replay.players[0].clone();
        timeline.replay.players = (0..10)
            .rev()
            .map(|slot| {
                let mut player = player_template.clone();
                player.slot = slot;
                player.hero_name = format!("npc_dota_hero_fixture_{slot}");
                player
            })
            .collect();
        for slot in 0..10 {
            let mut clip = mirana_clip(ClipCameraMode::PlayerPerspective);
            clip.view_hero = Some(format!("npc_dota_hero_fixture_{slot}"));
            assert_eq!(hero_slot_for_clip(&timeline, &clip), Some(slot));
            assert_eq!(
                camera_commands_for_clip(&timeline, &clip).first(),
                Some(&format!("dota_spectator_hero_index {slot}"))
            );
        }
    }

    #[test]
    fn selected_hero_modes_are_reapplied_only_inside_existing_preroll() {
        for mode in [ClipCameraMode::PlayerPerspective, ClipCameraMode::HeroChase] {
            assert_eq!(
                super::camera_reassert_frame_target(&mode, 60, 120),
                Some(30)
            );
            assert_eq!(super::camera_reassert_frame_target(&mode, 5, 65), Some(4));
            assert_eq!(super::camera_reassert_frame_target(&mode, 1, 61), Some(0));
            assert_eq!(super::camera_reassert_frame_target(&mode, 0, 60), Some(0));
        }
        for mode in [ClipCameraMode::Directed, ClipCameraMode::FreeCamera] {
            assert_eq!(super::camera_reassert_frame_target(&mode, 60, 120), None);
        }
    }

    fn mirana_timeline() -> TimelineDocument {
        TimelineDocument {
            schema_version: "1.1".to_string(),
            source_sha256: "abc".to_string(),
            parser: ParserIdentity {
                name: "fixture".to_string(),
                version: "1".to_string(),
            },
            replay: ReplayMetadata {
                playback_ticks: 30,
                playback_time_seconds: 1.0,
                game_build: 42,
                match_id: None,
                game_mode: None,
                game_winner: None,
                players: vec![ReplayPlayer {
                    slot: 7,
                    hero_name: "npc_dota_hero_mirana".to_string(),
                    player_name: Some("猫猫只用虎".to_string()),
                    game_team: Some(3),
                    is_fake_client: false,
                }],
            },
            events: Vec::new(),
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
            state_snapshots: Vec::new(),
        }
    }

    fn mirana_clip(camera_mode: ClipCameraMode) -> RenderClip {
        RenderClip {
            clip_id: "clip-001".to_string(),
            candidate_id: "hl-001".to_string(),
            view_hero: Some("npc_dota_hero_mirana".to_string()),
            camera_mode,
            take_group_id: None,
            take_role: RenderTakeRole::Primary,
            include_in_final: true,
            source_start_seconds: 10.0,
            source_peak_seconds: 12.0,
            source_end_seconds: 14.0,
            anchor_seconds: 12.0,
            anchor_tick: 360,
        }
    }

    #[test]
    fn clean_feed_hides_each_dota_hud_layer() {
        let commands = hud_commands(true);

        for expected in [
            "dota_spectator_options_enabled 0",
            "dota_spectator_hudhide",
            "dota_hud_hide_mainhud 1",
            "dota_hud_hide_topbar 1",
            "dota_hud_hide_minimap 1",
            "dota_hide_cursor 1",
            "r_drawpanorama 0",
        ] {
            assert!(commands.iter().any(|command| command == expected));
        }
    }

    #[test]
    fn dota_start_does_not_override_user_hud_preferences() {
        assert_eq!(
            super::dota_launch_args(),
            ["-insecure", "-vconsole", "-console", "-novid"]
        );
    }

    #[test]
    fn ordinary_feed_hides_only_replay_controls() {
        assert_eq!(
            hud_commands(false),
            vec!["dota_spectator_options_enabled 0".to_string()]
        );
    }

    #[test]
    fn recording_requires_steam_without_claiming_an_existing_dota() {
        assert!(matches!(
            super::validate_recording_clients(false, false),
            Err(super::RenderError::SteamRequiredForRecording)
        ));
        assert!(super::validate_recording_clients(false, true).is_ok());
        for steam_running in [false, true] {
            assert!(matches!(
                super::validate_recording_clients(true, steam_running),
                Err(super::RenderError::DotaAlreadyRunning)
            ));
        }
    }

    #[test]
    fn capture_pipeline_revision_invalidates_segment_cache() {
        let temp = tempfile::tempdir().unwrap();
        let request = RenderRequest {
            job_id: "d2h-cache-test".to_string(),
            source_sha256: "fixture-sha256".to_string(),
            job_dir: temp.path().to_path_buf(),
            source_replay: temp.path().join("fixture.dem"),
            dota2_exe: temp.path().join("dota2.exe"),
            ffmpeg_exe: temp.path().join("ffmpeg.exe"),
            ffprobe_exe: temp.path().join("ffprobe.exe"),
            timeline: mirana_timeline(),
            clips: vec![mirana_clip(ClipCameraMode::PlayerPerspective)],
            settings: RenderSettings::default(),
            resume_fingerprint: None,
        };

        let previous =
            super::render_fingerprint_with_pipeline(&request, "d2h.capture/previous").unwrap();
        let current =
            super::render_fingerprint_with_pipeline(&request, "d2h.capture/current").unwrap();

        assert_ne!(previous, current);
    }

    #[test]
    fn volume_parser_reads_ffmpeg_output() {
        let text = "[Parsed_volumedetect] mean_volume: -14.2 dB\nmax_volume: -0.8 dB";
        assert_eq!(parse_db_value(text, "mean_volume:"), Some(-14.2));
        assert_eq!(parse_db_value(text, "max_volume:"), Some(-0.8));
    }

    #[test]
    #[ignore = "requires local FFmpeg and a D2H_NATIVE_PROBE video"]
    fn native_probe_composes_with_original_bgm_and_passes_media_contract() {
        let source = std::path::PathBuf::from(
            std::env::var_os("D2H_NATIVE_PROBE").expect("D2H_NATIVE_PROBE is required"),
        );
        let ffmpeg = std::path::PathBuf::from(
            std::env::var_os("FFMPEG_EXE").unwrap_or_else(|| "ffmpeg.exe".into()),
        );
        let ffprobe = std::path::PathBuf::from(
            std::env::var_os("FFPROBE_EXE").unwrap_or_else(|| "ffprobe.exe".into()),
        );
        let temp = tempfile::tempdir().unwrap();
        let joined = temp.path().join("joined.mp4");
        concat_segments(&ffmpeg, &[source.clone(), source], temp.path(), &joined).unwrap();
        let joined_duration = probe_media(&ffprobe, &joined).unwrap().duration_seconds;
        let bgm = temp.path().join("bgm.wav");
        super::write_original_bgm(&bgm, joined_duration + 0.5, &[1.5, 4.5]).unwrap();
        let output = temp.path().join("completed.mp4");
        mix_final_audio(
            &ffmpeg,
            &joined,
            Some(&bgm),
            None,
            &output,
            joined_duration,
            &RenderSettings::default(),
        )
        .unwrap();
        let qc = run_qc(&ffmpeg, &ffprobe, &output).unwrap();

        assert!(qc.duration_seconds >= 5.9);
        assert_eq!((qc.width, qc.height), (1920, 1080));
        assert!(qc.has_video);
        assert!(qc.has_audio);
        assert_eq!(qc.black_events, 0);
    }

    #[test]
    #[ignore = "requires local FFmpeg; uses synthetic media, never opens Dota"]
    fn native_encoder_preserves_resolution_rate_and_standard_color() {
        let ffmpeg = std::path::PathBuf::from(
            std::env::var_os("FFMPEG_EXE").unwrap_or_else(|| "ffmpeg.exe".into()),
        );
        let ffprobe = std::path::PathBuf::from(
            std::env::var_os("FFPROBE_EXE").unwrap_or_else(|| "ffprobe.exe".into()),
        );
        let temp = tempfile::tempdir().unwrap();
        let raw = temp.path().join("raw");
        std::fs::create_dir(&raw).unwrap();
        let pattern = raw.join("frame-%06d.jpg");
        let wav = raw.join("game.wav");
        super::run_media_command(
            &ffmpeg,
            &[
                "-y".into(),
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                "testsrc2=size=2560x1440:rate=60:duration=1".into(),
                "-q:v".into(),
                "2".into(),
                super::path_arg(&pattern),
            ],
            "synthetic native-resolution frames",
        )
        .unwrap();
        super::run_media_command(
            &ffmpeg,
            &[
                "-y".into(),
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                "sine=frequency=440:sample_rate=48000:duration=1".into(),
                super::path_arg(&wav),
            ],
            "synthetic audio",
        )
        .unwrap();
        let output = temp.path().join("native-quality.mp4");
        encode_frame_sequence(&ffmpeg, &raw, &wav, &output, 1.0, 0.0).unwrap();
        let media = probe_media(&ffprobe, &output).unwrap();
        assert_eq!((media.width, media.height), (2560, 1440));
        assert_eq!(media.video_frames, Some(60));
        assert_eq!(media.frame_rate, 60.0);
        super::validate_segment_media(&media, 1.0).unwrap();
        let metadata = super::background_command(&ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=color_range,color_space,color_transfer,color_primaries,pix_fmt",
                "-of",
                "json",
            ])
            .arg(&output)
            .output()
            .unwrap();
        assert!(metadata.status.success());
        let value: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
        let video = &value["streams"][0];
        assert_eq!(video["color_range"], "tv");
        assert_eq!(video["color_space"], "bt709");
        assert_eq!(video["color_transfer"], "bt709");
        assert_eq!(video["color_primaries"], "bt709");
        assert_eq!(video["pix_fmt"], "yuv420p");
    }

    #[test]
    #[ignore = "requires local FFmpeg and a D2H_NATIVE_FRAME_DIR capture"]
    fn native_frame_capture_encodes_without_reopening_dota() {
        let raw_dir = std::path::PathBuf::from(
            std::env::var_os("D2H_NATIVE_FRAME_DIR").expect("D2H_NATIVE_FRAME_DIR is required"),
        );
        let ffmpeg = std::path::PathBuf::from(
            std::env::var_os("FFMPEG_EXE").unwrap_or_else(|| "ffmpeg.exe".into()),
        );
        let ffprobe = std::path::PathBuf::from(
            std::env::var_os("FFPROBE_EXE").unwrap_or_else(|| "ffprobe.exe".into()),
        );
        let frame_count = std::fs::read_dir(&raw_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| super::is_jpeg(&entry.path()))
            .count();
        let expected_duration = frame_count as f32 / super::FRAME_RATE as f32;
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("native-frame-capture.mp4");

        encode_frame_sequence(
            &ffmpeg,
            &raw_dir,
            &raw_dir.join("game.wav"),
            &output,
            expected_duration,
            super::CAPTURE_PREROLL_SECONDS,
        )
        .unwrap();
        let media = probe_media(&ffprobe, &output).unwrap();

        assert!((media.duration_seconds - expected_duration).abs() <= 0.1);
        assert_eq!((media.width, media.height), (1920, 1080));
        assert!(media.has_video);
        assert!(media.has_audio);
    }
}
