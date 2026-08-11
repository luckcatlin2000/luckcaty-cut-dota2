mod audio;

use audio::write_original_bgm;
use d2_highlights_core::TimelineDocument;
use d2_highlights_replay_control::{ReplayControlError, execute_vconsole_commands, probe_vconsole};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
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
const CAPTURE_PIPELINE_VERSION: &str = "d2h.capture/1.9.6-native-hud-12";
const FRAME_RATE: u32 = 30;
const CAPTURE_PREROLL_SECONDS: f32 = 1.0;
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
    let first = build().output()?;
    if is_dll_init_failure(first.status.code()) {
        thread::sleep(Duration::from_millis(300));
        build().output()
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
        "Steam 未运行。普通画面的 Dota 2 原生玩家昵称会显示为‘未知’。请手动打开并登录 Steam，等待连接稳定后再重试；本软件不会自动启动或登录 Steam。"
    )]
    SteamRequiredForNativeNames,
    #[error("找不到或无法运行 {0}。")]
    MissingTool(String),
    #[error("录像文件不存在：{0}")]
    MissingReplay(String),
    #[error("剪辑方案无效：{0}")]
    InvalidPlan(String),
    #[error("Dota 2 启动后未能建立离线控制连接。")]
    VConsoleUnavailable,
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
        "正在检查 Dota 2、编码器和剪辑方案",
        2,
        0,
        request.clips.len(),
    );
    check_cancelled(&cancellation)?;
    check_command(&request.ffmpeg_exe, &["-version"], "FFmpeg")?;
    check_command(&request.ffprobe_exe, &["-version"], "FFprobe")?;
    if dota_is_running()? {
        return Err(RenderError::DotaAlreadyRunning);
    }
    if native_player_names_require_steam(request.settings.clean_hud) && !steam_is_running()? {
        return Err(RenderError::SteamRequiredForNativeNames);
    }

    let fingerprint = render_fingerprint(&request)?;
    let cache_root = request
        .job_dir
        .join("render")
        .join("cache")
        .join(&fingerprint[..16]);
    let segment_root = cache_root.join("segments");
    let state_path = cache_root.join("state.json");
    fs::create_dir_all(&segment_root)?;
    let mut state = load_or_create_state(&state_path, &fingerprint)?;
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
        &fingerprint,
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
    let mut child = launch_dota(&request.dota2_exe)?;
    let session_result = run_dota_session(
        &request,
        &prepared_replay.replay_reference,
        &movie_dir,
        &segment_root,
        &state_path,
        &mut state,
        &cancellation,
        &mut child,
        &mut on_progress,
    );
    let shutdown_warning = shutdown_dota(&mut child);
    if prepared_replay.created_by_app {
        let _ = fs::remove_file(&prepared_replay.path);
    }

    let segments = session_result?;
    let mut warnings = Vec::new();
    if let Some(warning) = shutdown_warning {
        warnings.push(warning);
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

    let final_path = output_dir.join(format!("{output_stem}.mp4"));
    mix_final_audio(
        &request.ffmpeg_exe,
        &joined_path,
        bgm_path.as_deref(),
        narration_path.as_deref(),
        &final_path,
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
    let mut qc = run_qc(&request.ffmpeg_exe, &request.ffprobe_exe, &final_path)?;
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
        "成片已完成，Dota 2 已关闭",
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
        fs::copy(segment, &output_path)?;
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
    cancellation: &CancellationToken,
    child: &mut Child,
    on_progress: &mut F,
) -> Result<Vec<PathBuf>, RenderError>
where
    F: FnMut(RenderProgress),
{
    wait_for_vconsole(child, cancellation)?;
    check_cancelled(cancellation)?;
    send_commands(&[format!("playdemo {replay_reference}")])?;
    wait_with_cancel(Duration::from_secs(12), cancellation, child, |_| {})?;

    let total = request.clips.len();
    let mut rendered_segments = Vec::with_capacity(total);
    for (index, clip) in request.clips.iter().enumerate() {
        check_cancelled(cancellation)?;
        let output_path = segment_root.join(format!("{:02}-{}.mp4", index + 1, clip.clip_id));
        if segment_cache_is_valid(&request.ffprobe_exe, &output_path) {
            let duration = probe_media(&request.ffprobe_exe, &output_path)?.duration_seconds;
            state.segments.insert(
                clip.clip_id.clone(),
                SegmentState {
                    candidate_id: clip.clip_id.clone(),
                    output_path: output_path.display().to_string(),
                    duration_seconds: duration,
                    status: "complete".to_string(),
                },
            );
            write_json(state_path, state)?;
            rendered_segments.push(output_path);
            emit(
                on_progress,
                "capture",
                "running",
                &format!("已恢复片段 {} 的缓存", index + 1),
                clip_percent(index + 1, total),
                index + 1,
                total,
            );
            continue;
        }

        emit(
            on_progress,
            "capture",
            "running",
            &format!("正在导出第 {}/{} 段：{}", index + 1, total, clip.clip_id),
            clip_percent(index, total),
            index + 1,
            total,
        );
        let clip_dir = segment_root.join(format!("{:02}-{}", index + 1, clip.clip_id));
        fs::create_dir_all(&clip_dir)?;
        let base_path = clip_dir.join("base.mp4");
        capture_clip(
            request,
            clip,
            index,
            movie_dir,
            &clip_dir,
            &base_path,
            cancellation,
            child,
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
                    &format!("第 {}/{} 段正在离线导出画面与声音", index + 1, total),
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
            &output_path,
            clip,
            request.settings.replay_emphasis,
        )?;
        let duration = probe_media(&request.ffprobe_exe, &output_path)?.duration_seconds;
        state.segments.insert(
            clip.clip_id.clone(),
            SegmentState {
                candidate_id: clip.clip_id.clone(),
                output_path: output_path.display().to_string(),
                duration_seconds: duration,
                status: "complete".to_string(),
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
    mut on_tick: P,
) -> Result<(), RenderError>
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

    send_commands(&["demo_resume".to_string()])?;
    thread::sleep(Duration::from_millis(350));
    send_commands(&[format!("demo_goto {capture_start_tick} absolute pause")])?;
    wait_for_seek(capture_start_tick, cancellation, child)?;

    let mut setup_commands = vec!["demo_timescale 1.000".to_string()];
    setup_commands.extend(camera_commands_for_clip(&request.timeline, clip));
    setup_commands.extend(hud_commands(request.settings.clean_hud));
    send_commands(&setup_commands)?;
    wait_with_cancel(Duration::from_millis(250), cancellation, child, |_| {})?;

    let before = collect_movie_files(movie_dir)?;
    let prefix = format!(
        "d2h_{}_{}_{}",
        index + 1,
        clip.clip_id.replace('-', "_"),
        unix_seconds()?
    );
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
    let impact_start_frame = preroll_frame_count + frame_count_for_duration(impact_start);
    let impact_output_end_frame = preroll_frame_count
        + frame_count_for_duration(impact_start + (impact_end - impact_start) / slow_speed);

    send_commands(&[
        format!("startmovie {prefix} jpg wav jpeg_quality 95 framerate {FRAME_RATE}"),
        format!("demo_pauseatservertick {end_tick}"),
        "demo_resume".to_string(),
    ])?;
    let started = Instant::now();
    if slow_speed < 1.0 && impact_end > impact_start {
        wait_for_capture_frame_count(
            movie_dir,
            &before,
            &prefix,
            impact_start_frame,
            capture_frame_count,
            started,
            cancellation,
            child,
            &mut on_tick,
        )?;
        let impact_commands = vec![format!("demo_timescale {slow_speed:.3}")];
        send_commands(&impact_commands)?;
        wait_for_capture_frame_count(
            movie_dir,
            &before,
            &prefix,
            impact_output_end_frame,
            capture_frame_count,
            started,
            cancellation,
            child,
            &mut on_tick,
        )?;
        let result_commands = vec!["demo_timescale 1.000".to_string()];
        send_commands(&result_commands)?;
    }
    wait_for_capture_frame_count(
        movie_dir,
        &before,
        &prefix,
        capture_frame_count,
        capture_frame_count,
        started,
        cancellation,
        child,
        &mut on_tick,
    )?;
    let _ = send_commands(&["endmovie".to_string(), "demo_pause".to_string()]);

    let raw_dir = clip_dir.join("raw");
    fs::create_dir_all(&raw_dir)?;
    let captured = wait_for_movie_files(movie_dir, &before, &prefix, cancellation)?;
    let captured_frame_count = captured.iter().filter(|path| is_jpeg(path)).count();
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
    Ok(())
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
        fs::copy(input, output)?;
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
        fs::copy(input, output)?;
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
        "18".to_string(),
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
            "scale=1920:1080:force_original_aspect_ratio=decrease:flags=lanczos,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:black,fps=30,format=yuv420p".to_string(),
            "-c:v".to_string(),
            "libx264".to_string(),
            "-preset".to_string(),
            "medium".to_string(),
            "-crf".to_string(),
            "18".to_string(),
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
    let qc_output = output_with_loader_retry(|| {
        let mut command = background_command(ffmpeg);
        command.args([
            "-hide_banner",
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
    let stderr = String::from_utf8_lossy(&qc_output.stderr);
    let black_events = stderr.matches("black_start:").count();
    let freeze_events = stderr.matches("freeze_start:").count();
    let audio_mean_db = parse_db_value(&stderr, "mean_volume:");
    let audio_peak_db = parse_db_value(&stderr, "max_volume:");
    let mut warnings = Vec::new();
    if black_events > 0 {
        warnings.push(format!("检测到 {black_events} 个超过 0.5 秒的黑帧区间。"));
    }
    if freeze_events > 0 {
        warnings.push(format!("检测到 {freeze_events} 个超过 1.5 秒的冻结区间。"));
    }
    if !media.has_audio {
        warnings.push("最终成片没有音轨。".to_string());
    }
    if media.width != 1920 || media.height != 1080 {
        warnings.push(format!(
            "最终分辨率为 {}x{}，不是目标 1920x1080。",
            media.width, media.height
        ));
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
}

fn probe_media(ffprobe: &Path, path: &Path) -> Result<MediaInfo, RenderError> {
    let output = output_with_loader_retry(|| {
        let mut command = background_command(ffprobe);
        command.args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,width,height",
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
    if let Some(streams) = value.get("streams").and_then(|value| value.as_array()) {
        for stream in streams {
            match stream.get("codec_type").and_then(|value| value.as_str()) {
                Some("video") => {
                    has_video = true;
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
    })
}

fn validate_request(request: &RenderRequest) -> Result<(), RenderError> {
    if !request.source_replay.is_file() {
        return Err(RenderError::MissingReplay(
            request.source_replay.display().to_string(),
        ));
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
        if !ids.insert(&clip.clip_id) {
            return Err(RenderError::InvalidPlan(format!(
                "片段 {} 重复出现",
                clip.clip_id
            )));
        }
        let duration = clip.source_end_seconds - clip.source_start_seconds;
        if !duration.is_finite() || !(1.0..=100.0).contains(&duration) {
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
        let state: RenderState = serde_json::from_slice(&fs::read(path)?)?;
        if state.fingerprint == fingerprint {
            return Ok(state);
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
    fingerprint: &str,
) -> Result<PreparedReplay, RenderError> {
    let source_canonical = fs::canonicalize(source)?;
    let replay_canonical = fs::canonicalize(replay_dir)?;
    let (path, created_by_app) = if source_canonical
        .parent()
        .is_some_and(|parent| parent == replay_canonical)
    {
        (source_canonical, false)
    } else {
        let safe_job = job_id
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .take(18)
            .collect::<String>();
        let destination = replay_dir.join(format!("d2h_{}_{}.dem", safe_job, &fingerprint[..10]));
        if !destination.is_file()
            || fs::metadata(&destination)?.len() != fs::metadata(&source_canonical)?.len()
        {
            fs::copy(&source_canonical, &destination)?;
        }
        (destination, true)
    };
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| RenderError::InvalidPlan("录像文件名无法用于回放。".to_string()))?
        .to_string();
    Ok(PreparedReplay {
        path,
        replay_reference: format!("replays/{stem}"),
        created_by_app,
    })
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
    cancellation: &CancellationToken,
) -> Result<(), RenderError> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(90) {
        check_cancelled(cancellation)?;
        if child.try_wait()?.is_some() {
            return Err(RenderError::DotaExited);
        }
        if probe_vconsole(Duration::from_secs(2)).is_ok() {
            // Dota can fatally race a second VConsole connection while the probe socket closes.
            thread::sleep(Duration::from_secs(1));
            return Ok(());
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err(RenderError::VConsoleUnavailable)
}

fn wait_for_seek(
    target_tick: u32,
    cancellation: &CancellationToken,
    child: &mut Child,
) -> Result<(), RenderError> {
    let started = Instant::now();
    let mut had_response = false;
    while started.elapsed() < Duration::from_secs(15) {
        check_cancelled(cancellation)?;
        if child.try_wait()?.is_some() {
            return Err(RenderError::DotaExited);
        }
        if let Ok(report) =
            execute_vconsole_commands(&["demo_info".to_string()], VCONSOLE_COMMAND_TIMEOUT)
        {
            had_response = true;
            if report
                .console_excerpt
                .iter()
                .flat_map(|line| line.split(|character: char| !character.is_ascii_digit()))
                .filter_map(|value| value.parse::<u32>().ok())
                .any(|tick| tick.abs_diff(target_tick) <= 5)
            {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(650));
    }
    if had_response {
        Ok(())
    } else {
        Err(RenderError::ReplayControl(
            "回放跳转后没有收到状态回应。".to_string(),
        ))
    }
}

fn send_commands(commands: &[String]) -> Result<(), RenderError> {
    execute_vconsole_commands(commands, VCONSOLE_COMMAND_TIMEOUT)
        .map(|_| ())
        .map_err(|error| RenderError::ReplayControl(error.to_string()))
}

fn shutdown_dota(child: &mut Child) -> Option<String> {
    let _ = execute_vconsole_commands(
        &[
            "endmovie".to_string(),
            "demo_pause".to_string(),
            "cl_drawhud 1".to_string(),
            "dota_hide_cursor 0".to_string(),
        ],
        Duration::from_secs(2),
    );
    let pid = child.id().to_string();
    #[cfg(windows)]
    {
        let _ = background_command("taskkill")
            .args(["/PID", &pid, "/T"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(8) {
        match child.try_wait() {
            Ok(Some(_)) => return None,
            Ok(None) => thread::sleep(Duration::from_millis(250)),
            Err(error) => return Some(format!("关闭 Dota 2 状态检查失败：{error}")),
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

fn native_player_names_require_steam(clean_hud: bool) -> bool {
    !clean_hud
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
    let output = background_command("tasklist")
        .args([
            "/FI",
            &format!("IMAGENAME eq {image_name}"),
            "/FO",
            "CSV",
            "/NH",
        ])
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .to_ascii_lowercase()
        .contains(&image_name.to_ascii_lowercase()))
}

fn collect_movie_files(root: &Path) -> Result<BTreeMap<PathBuf, u64>, RenderError> {
    fn visit(path: &Path, files: &mut BTreeMap<PathBuf, u64>) -> io::Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit(&path, files)?;
            } else {
                files.insert(path, entry.metadata()?.len());
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    if root.is_dir() {
        visit(root, &mut files)?;
    }
    Ok(files)
}

fn wait_for_movie_files(
    movie_dir: &Path,
    before: &BTreeMap<PathBuf, u64>,
    prefix: &str,
    cancellation: &CancellationToken,
) -> Result<Vec<PathBuf>, RenderError> {
    let started = Instant::now();
    let mut last_count = 0;
    let mut stable_since = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        check_cancelled(cancellation)?;
        let current = collect_movie_files(movie_dir)?;
        let mut captured = current
            .iter()
            .filter(|(path, size)| {
                let is_new = before.get(*path).is_none_or(|old_size| old_size != *size);
                let name_matches = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.contains(prefix));
                is_new && (name_matches || before.is_empty())
            })
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        captured.sort();
        if captured.len() != last_count {
            last_count = captured.len();
            stable_since = Instant::now();
        } else if captured.len() >= FRAME_RATE as usize
            && stable_since.elapsed() >= Duration::from_secs(2)
        {
            return Ok(captured);
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
        fs::copy(source, &destination)?;
        copied.push(destination);
    }
    let wav_destination = raw_dir.join("game.wav");
    fs::copy(wav, &wav_destination)?;
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
            }
            commands.push("dota_spectator_mode 2".to_string());
            if let Some(slot) = slot {
                commands.push(format!("dota_camera_focus_player {slot}"));
            }
            commands
        }
        ClipCameraMode::PlayerPerspective => {
            let mut commands = Vec::new();
            if let Some(slot) = slot {
                commands.push(format!("dota_spectator_hero_index {slot}"));
            }
            commands.push("dota_spectator_mode 3".to_string());
            if let Some(slot) = slot {
                commands.push(format!("dota_camera_focus_player {slot}"));
            }
            commands
        }
    }
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

#[allow(clippy::too_many_arguments)]
fn wait_for_capture_frame_count<P>(
    movie_dir: &Path,
    before: &BTreeMap<PathBuf, u64>,
    prefix: &str,
    target_frames: usize,
    total_frames: usize,
    started: Instant,
    cancellation: &CancellationToken,
    child: &mut Child,
    on_tick: &mut P,
) -> Result<(), RenderError>
where
    P: FnMut(f32, f32),
{
    let total_seconds = total_frames as f32 / FRAME_RATE as f32;
    let timeout_seconds = (total_seconds * 6.0 + 30.0).clamp(45.0, 900.0);
    let timeout = Duration::from_secs_f32(timeout_seconds);
    let mut captured_frames = 0;
    while started.elapsed() < timeout {
        check_cancelled(cancellation)?;
        if child.try_wait()?.is_some() {
            return Err(RenderError::DotaExited);
        }
        captured_frames = collect_movie_files(movie_dir)?
            .iter()
            .filter(|(path, size)| capture_file_matches(path, **size, before, prefix))
            .filter(|(path, _)| is_jpeg(path))
            .count();
        let captured_seconds = captured_frames.min(total_frames) as f32 / FRAME_RATE as f32;
        on_tick(captured_seconds, total_seconds);
        if captured_frames >= target_frames {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(RenderError::Media(format!(
        "Dota 2 原生逐帧导出超时：需要 {target_frames} 帧，只得到 {captured_frames} 帧"
    )))
}

fn capture_file_matches(
    path: &Path,
    size: u64,
    before: &BTreeMap<PathBuf, u64>,
    prefix: &str,
) -> bool {
    let is_new = before.get(path).is_none_or(|old_size| *old_size != size);
    let name_matches = path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.contains(prefix));
    is_new && name_matches
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

fn segment_cache_is_valid(ffprobe: &Path, path: &Path) -> bool {
    path.is_file()
        && fs::metadata(path).is_ok_and(|metadata| metadata.len() > 1024)
        && probe_media(ffprobe, path)
            .is_ok_and(|media| media.duration_seconds >= 1.0 && media.has_video && media.has_audio)
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
    let output = background_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("D2H_NARRATION_PATH", path)
        .env("D2H_NARRATION_TEXT", text)
        .output()
        .map_err(|error| error.to_string())?;
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
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
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
        let replay = std::path::Path::new(r"C:\replays\1234567890.dem");
        let stem = replay_output_stem(replay);
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
        assert_eq!(frame_count_for_duration(1.0 / 30.0), 1);
        assert_eq!(frame_count_for_duration(8.0), 240);
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
                "dota_spectator_mode 3",
                "dota_camera_focus_player 7",
            ]
        );
        assert_eq!(
            hero_chase,
            vec![
                "dota_spectator_hero_index 7",
                "dota_spectator_mode 2",
                "dota_camera_focus_player 7",
            ]
        );
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
                    player_name: Some("Player A".to_string()),
                    game_team: Some(3),
                    is_fake_client: false,
                }],
            },
            events: Vec::new(),
            tree_orders: Vec::new(),
            temporary_trees: Vec::new(),
            salutes: Vec::new(),
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
    fn ordinary_feed_requires_steam_for_native_player_names() {
        assert!(super::native_player_names_require_steam(false));
        assert!(!super::native_player_names_require_steam(true));
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
