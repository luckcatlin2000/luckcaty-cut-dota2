//! Independent factual WTF analysis and explicit, isolated recorder handoff.
use super::*;
use d2_highlights_core::{HighlightStory, StoryDocument, StoryShot, StoryTakeRole};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri_plugin_dialog::DialogExt;

const DRAFT_SCHEMA: &str = "maomao.wtf-draft/1";
const PLAN_LIMIT: usize = 256 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WtfClipInput {
    clip_id: String,
    story_id: String,
    shot_id: String,
    hero: String,
    start_seconds: f32,
    end_seconds: f32,
    camera_mode: ClipCameraMode,
    take_group_id: String,
    take_role: RenderTakeRole,
    include_in_final: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WtfDraft {
    schema_version: String,
    job_id: String,
    source_sha256: String,
    clips: Vec<WtfClipInput>,
    updated_unix_seconds: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PlanExport {
    plan_path: String,
    clip_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Handoff {
    session_directory: String,
    recording_job_id: String,
    clip_count: usize,
    launched: bool,
    message: String,
}

struct WtfJob {
    directory: PathBuf,
    manifest: JobManifest,
    timeline: TimelineDocument,
    highlights: HighlightDocument,
    stories: StoryDocument,
}

fn require_wtf(module: &DesktopModule) -> Result<(), String> {
    if matches!(module, DesktopModule::Wtf) {
        Ok(())
    } else {
        Err("此操作只在独立 WTF 导演中提供。".into())
    }
}

#[tauri::command]
pub(super) async fn analyze_wtf_dem(
    path: String,
    on_progress: Channel<AnalysisProgress>,
    state: State<'_, AppState>,
    module: State<'_, DesktopModule>,
) -> Result<AnalysisSummary, String> {
    require_wtf(module.inner())?;
    let lease = state.activities.begin(Activity::Analysis)?;
    let jobs_root = state.jobs_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let summary = d2_highlights_pipeline::analyze_dem_with_progress(
            Path::new(&path),
            &jobs_root,
            |progress| {
                let _ = on_progress.send(progress);
            },
        )
        .map_err(|error| error.to_string())?;
        let _ = mark_job_recent(&jobs_root, &summary.job_id);
        Ok(summary)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(super) async fn get_wtf_draft(
    job_id: String,
    state: State<'_, AppState>,
    module: State<'_, DesktopModule>,
) -> Result<Option<WtfDraft>, String> {
    require_wtf(module.inner())?;
    let lease = state.activities.begin(Activity::Save)?;
    let jobs_root = state.jobs_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let job = load_job(&jobs_root, &job_id)?;
        let path = job.directory.join("director/wtf-draft.json");
        if !path.exists() {
            return Ok(None);
        }
        let draft: WtfDraft = read_bound(&job.directory, &path, PLAN_LIMIT as u64)?;
        if draft.schema_version != DRAFT_SCHEMA
            || draft.job_id != job_id
            || draft.source_sha256 != job.manifest.source.sha256
        {
            return Err("已有 WTF 草稿与本局来源或版本不一致，原文件已保留。".into());
        }
        validate_clips(&job, &draft.clips, false)?;
        Ok(Some(draft))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(super) async fn save_wtf_draft(
    job_id: String,
    clips: Vec<WtfClipInput>,
    state: State<'_, AppState>,
    module: State<'_, DesktopModule>,
) -> Result<WtfDraft, String> {
    require_wtf(module.inner())?;
    let lease = state.activities.begin(Activity::Save)?;
    let jobs_root = state.jobs_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let job = load_job(&jobs_root, &job_id)?;
        save_draft(&job, clips)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(super) async fn export_wtf_plan(
    job_id: String,
    clips: Vec<WtfClipInput>,
    app: AppHandle,
    state: State<'_, AppState>,
    module: State<'_, DesktopModule>,
) -> Result<Option<PlanExport>, String> {
    require_wtf(module.inner())?;
    let lease = state.activities.begin(Activity::Export)?;
    let jobs_root = state.jobs_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let job = load_job(&jobs_root, &job_id)?;
        let plan = build_plan(&job, &clips)?;
        verify_source(&job)?;
        let selected = app
            .dialog()
            .file()
            .set_title("导出 WTF 录制方案")
            .add_filter("JSON 录制方案", &["json"])
            .set_file_name(format!(
                "WTF录制方案_{}_{}.json",
                job.timeline
                    .replay
                    .match_id
                    .map_or_else(|| "录像".into(), |id| id.to_string()),
                stamp()
            ))
            .blocking_save_file();
        let Some(selected) = selected else {
            return Ok(None);
        };
        let path = selected.into_path().map_err(|error| error.to_string())?;
        export_new(&path, &plan)?;
        Ok(Some(PlanExport {
            plan_path: path.display().to_string(),
            clip_count: clips.len(),
        }))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(super) async fn handoff_wtf_plan(
    job_id: String,
    clips: Vec<WtfClipInput>,
    state: State<'_, AppState>,
    module: State<'_, DesktopModule>,
) -> Result<Handoff, String> {
    require_wtf(module.inner())?;
    let lease = state.activities.begin(Activity::Export)?;
    let jobs_root = state.jobs_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let job = load_job(&jobs_root, &job_id)?;
        let (mut receipt, replay) = prepare_handoff(&job, &clips)?;
        match recorder_executable() {
            Ok(Some(executable)) => {
                let mut command = Command::new(executable);
                command.arg("--replay").arg(replay)
                    .env("D2H_PROJECT_ROOT", &receipt.session_directory);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    command.creation_flags(0x0800_0000);
                }
                match command.spawn() {
                    Ok(_) => {
                        receipt.launched = true;
                        receipt.message = "已在独立会话打开剪辑助手；请核对英雄、机位和时间后手动开始导出。".into();
                    }
                    Err(error) => receipt.message = format!("独立录制会话已保存，但剪辑助手未能打开：{error}。可稍后导入会话内的 plan.json。"),
                }
            }
            Ok(None) => receipt.message = "独立录制会话已保存，未找到相邻安装的剪辑助手。请在新版剪辑助手中导入会话内的 plan.json。".into(),
            Err(error) => receipt.message = format!("独立录制会话已保存，剪辑助手入口检查失败：{error}"),
        }
        let receipt_path = Path::new(&receipt.session_directory).join("handoff-receipt.json");
        let _ = d2_highlights_core::write_json_pretty(&receipt_path, &receipt);
        Ok(receipt)
    }).await.map_err(|error| error.to_string())?
}

fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn ordinary_under(root: &Path, target: &Path) -> Result<(), String> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| "WTF 文件路径超出本局任务。")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err("WTF 文件路径无效。".into());
        }
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| format!("WTF 任务文件无法访问：{error}"))?;
        if linked(&metadata) {
            return Err("不读取链接形式的 WTF 任务文件。".into());
        }
    }
    Ok(())
}

fn read_bound<T: serde::de::DeserializeOwned>(
    job: &Path,
    path: &Path,
    limit: u64,
) -> Result<T, String> {
    ordinary_under(job, path)?;
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > limit {
        return Err("WTF 任务文件超过读取上限。".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("WTF 任务文件在读取时超过上限。".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("WTF 任务文件无效，原文件已保留：{error}"))
}

fn load_job(root: &Path, job_id: &str) -> Result<WtfJob, String> {
    if !valid_job_id(job_id) {
        return Err("WTF 任务编号无效。".into());
    }
    let root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let directory = root.join(job_id);
    ordinary_under(&root, &directory)?;
    if !directory.is_dir() {
        return Err("WTF 任务不存在。".into());
    }
    let job = WtfJob {
        manifest: read_bound(&directory, &directory.join("manifest.json"), 1024 * 1024)?,
        timeline: read_bound(
            &directory,
            &directory.join("timeline/combat-events.json"),
            256 * 1024 * 1024,
        )?,
        highlights: read_bound(
            &directory,
            &directory.join("timeline/highlights.json"),
            32 * 1024 * 1024,
        )?,
        stories: read_bound(
            &directory,
            &directory.join("director/stories.json"),
            32 * 1024 * 1024,
        )?,
        directory,
    };
    if job.manifest.job_id != job_id
        || job.manifest.source.sha256 != job.timeline.source_sha256
        || job.manifest.source.sha256 != job.highlights.source_sha256
        || job.manifest.source.sha256 != job.stories.source_sha256
        || job.timeline.schema_version != d2_highlights_core::TIMELINE_SCHEMA_VERSION
        || job.stories.schema_version != d2_highlights_core::STORY_SCHEMA_VERSION
    {
        return Err("WTF 故事、事件与录像来源不一致，请重新分析；已有草稿保留。".into());
    }
    Ok(job)
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn source_shot<'a>(
    job: &'a WtfJob,
    clip: &WtfClipInput,
) -> Result<(&'a HighlightStory, &'a StoryShot), String> {
    let story = job
        .stories
        .stories
        .iter()
        .find(|story| story.id == clip.story_id)
        .ok_or("WTF 故事不属于本局录像。")?;
    let shot = story
        .shots
        .iter()
        .find(|shot| shot.id == clip.shot_id)
        .ok_or("WTF 机位不属于所选故事。")?;
    if !story.candidate_ids.contains(&shot.candidate_id)
        || !job
            .highlights
            .candidates
            .iter()
            .any(|candidate| candidate.id == shot.candidate_id)
    {
        return Err("WTF 机位缺少本局真实候选来源。".into());
    }
    Ok((story, shot))
}

fn validate_clips(job: &WtfJob, clips: &[WtfClipInput], nonempty: bool) -> Result<(), String> {
    if clips.len() > 100 || (nonempty && clips.is_empty()) {
        return Err("请选择1至100个 WTF 机位片段。".into());
    }
    let duration = job.timeline.replay.playback_time_seconds;
    let mut ids = HashSet::new();
    let mut shots = HashSet::new();
    let mut groups: HashMap<&str, Vec<&WtfClipInput>> = HashMap::new();
    for clip in clips {
        if !safe_id(&clip.clip_id)
            || !safe_id(&clip.take_group_id)
            || !ids.insert(&clip.clip_id)
            || !shots.insert((&clip.story_id, &clip.shot_id))
        {
            return Err("WTF 片段或机位编号无效、过长或重复。".into());
        }
        let (story, shot) = source_shot(job, clip)?;
        if !matches!(
            (clip.take_role, shot.take_role),
            (RenderTakeRole::Primary, StoryTakeRole::Primary)
                | (RenderTakeRole::Alternate, StoryTakeRole::Alternate)
        ) || clip.include_in_final != shot.include_in_default_cut
        {
            return Err("WTF 主备机位与默认入片设置必须保留原故事合同。".into());
        }
        if clip.take_group_id != format!("wtf-{}-{}", story.id, shot.take_group_id) {
            return Err("WTF 同步素材组与原故事机位不一致。".into());
        }
        if !matches!(
            clip.camera_mode,
            ClipCameraMode::PlayerPerspective | ClipCameraMode::HeroChase
        ) {
            return Err("WTF 录制仅支持玩家视角或英雄跟随。".into());
        }
        if !job
            .timeline
            .replay
            .players
            .iter()
            .any(|player| player.hero_name == clip.hero)
        {
            return Err("WTF 录制英雄不在本局阵容中。".into());
        }
        if !duration.is_finite()
            || !clip.start_seconds.is_finite()
            || !clip.end_seconds.is_finite()
            || clip.start_seconds < 0.0
            || clip.end_seconds > duration
            || clip.end_seconds - clip.start_seconds < 1.0
            || clip.end_seconds - clip.start_seconds > 100.0
        {
            return Err("WTF 片段必须在本局录像范围内，时长为1至100秒。".into());
        }
        groups.entry(&clip.take_group_id).or_default().push(clip);
    }
    for group in groups.values() {
        let primaries: Vec<_> = group
            .iter()
            .filter(|clip| clip.take_role == RenderTakeRole::Primary)
            .collect();
        if primaries.len() != 1
            || !primaries[0].include_in_final
            || group
                .iter()
                .any(|clip| clip.take_role == RenderTakeRole::Alternate && clip.include_in_final)
        {
            return Err("每组 WTF 素材必须恰有一个默认入片主机位；备用机位不增加成片时长。".into());
        }
        let primary = primaries[0];
        let (_, source) = source_shot(job, primary)?;
        for clip in group {
            let (_, other) = source_shot(job, clip)?;
            if (clip.start_seconds - primary.start_seconds).abs() > 0.0001
                || (clip.end_seconds - primary.end_seconds).abs() > 0.0001
                || source.candidate_id != other.candidate_id
            {
                return Err("同一 WTF 素材组的候选、入点和出点必须完全一致。".into());
            }
        }
    }
    Ok(())
}

fn save_draft(job: &WtfJob, clips: Vec<WtfClipInput>) -> Result<WtfDraft, String> {
    validate_clips(job, &clips, false)?;
    let draft = WtfDraft {
        schema_version: DRAFT_SCHEMA.into(),
        job_id: job.manifest.job_id.clone(),
        source_sha256: job.manifest.source.sha256.clone(),
        clips,
        updated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    let bytes = serde_json::to_vec(&draft).map_err(|error| error.to_string())?;
    if bytes.len() > PLAN_LIMIT {
        return Err("WTF 草稿超过256KiB，已有草稿保留。".into());
    }
    let director = job.directory.join("director");
    ordinary_under(&job.directory, &director)?;
    let path = director.join("wtf-draft.json");
    if path.exists() {
        let previous: WtfDraft = read_bound(&job.directory, &path, PLAN_LIMIT as u64)?;
        if previous.schema_version != DRAFT_SCHEMA
            || previous.job_id != job.manifest.job_id
            || previous.source_sha256 != job.manifest.source.sha256
        {
            return Err("已有 WTF 草稿的来源或版本不一致，不能覆盖。".into());
        }
    }
    d2_highlights_core::write_json_pretty(&path, &draft)
        .map_err(|error| format!("WTF 草稿保存失败，原文件保留：{error}"))?;
    Ok(draft)
}

fn clock(seconds: f32) -> String {
    let ms = (seconds as f64 * 1000.0).round().max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

fn build_plan(job: &WtfJob, clips: &[WtfClipInput]) -> Result<serde_json::Value, String> {
    validate_clips(job, clips, true)?;
    let match_id = job
        .timeline
        .replay
        .match_id
        .ok_or("本局缺少录像编号，无法导出 WTF 方案。")?;
    let mut anchors = HashMap::new();
    let mut rows = Vec::new();
    for clip in clips {
        // The recorder edits on a 30 Hz grid. Quantize first, then find a real
        // event inside that exact window; an original float boundary may move.
        let start = (clip.start_seconds * 30.0).round() / 30.0;
        let end = (clip.end_seconds * 30.0).round() / 30.0;
        let (story, shot) = source_shot(job, clip)?;
        let candidate = job
            .highlights
            .candidates
            .iter()
            .find(|candidate| candidate.id == shot.candidate_id)
            .ok_or("WTF 候选来源缺失。")?;
        let anchor = if let Some(anchor) = anchors.get(&clip.take_group_id) {
            *anchor
        } else {
            let event = job
                .timeline
                .events
                .iter()
                .filter(|event| {
                    event
                        .time_seconds
                        .is_some_and(|seconds| seconds >= start && seconds <= end)
                })
                .min_by(|left, right| {
                    let score = |event: &d2_highlights_core::CombatEvent| {
                        (
                            if event.tick == candidate.anchor_tick {
                                0
                            } else {
                                1
                            },
                            (event.time_seconds.unwrap_or_default() - candidate.peak_seconds).abs(),
                        )
                    };
                    let a = score(left);
                    let b = score(right);
                    a.0.cmp(&b.0)
                        .then_with(|| a.1.total_cmp(&b.1))
                        .then_with(|| left.tick.cmp(&right.tick))
                })
                .ok_or("WTF 片段内没有可验证的真实事件锚点，请调整时间范围。")?;
            let anchor = (event.tick, event.time_seconds.ok_or("WTF 事件缺少时间。")?);
            anchors.insert(clip.take_group_id.clone(), anchor);
            anchor
        };
        rows.push(serde_json::json!({
            "id": clip.clip_id, "title": format!("{} · {}", story.title, if clip.camera_mode == ClipCameraMode::PlayerPerspective { "玩家视角" } else { "英雄跟随" }),
            "start": clock(start), "end": clock(end), "hero": clip.hero,
            "anchor": { "demo_tick": anchor.0, "event_time_seconds": anchor.1 },
            "camera_mode": clip.camera_mode, "take_group_id": clip.take_group_id,
            "take_role": clip.take_role, "include_in_final": clip.include_in_final,
        }));
    }
    let plan = serde_json::json!({"schema_version":"d2h.timestamp-plan/1.2", "match_id":match_id.to_string(), "hero":clips[0].hero, "time_basis":"demo", "source":{"sha256":job.manifest.source.sha256}, "clips":rows});
    if serde_json::to_vec(&plan)
        .map_err(|error| error.to_string())?
        .len()
        > PLAN_LIMIT
    {
        return Err("WTF 录制方案超过256KiB。".into());
    }
    timestamp_import::build_preview(
        &plan.to_string(),
        "WTF录制方案.json",
        &job.manifest.source,
        &job.timeline,
        None,
        None,
    )
    .map_err(|error| format!("WTF 方案未通过剪辑助手导入校验：{error}"))?;
    Ok(plan)
}

fn verify_source(job: &WtfJob) -> Result<PathBuf, String> {
    let source = d2_highlights_core::inspect_dem(Path::new(&job.manifest.source.path))
        .map_err(|error| format!("原始录像无法核对：{error}"))?;
    if source.sha256 != job.manifest.source.sha256 {
        return Err("原始录像内容已改变，不能交接旧 WTF 方案。".into());
    }
    Ok(PathBuf::from(source.path))
}

fn export_new(path: &Path, plan: &serde_json::Value) -> Result<(), String> {
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Err("请将 WTF 录制方案保存为 .json 文件。".into());
    }
    let mut bytes = serde_json::to_vec_pretty(plan).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() > PLAN_LIMIT {
        return Err("WTF 录制方案超过256KiB。".into());
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("无法创建方案；同名文件不会覆盖，请选择新文件名：{error}"))?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(format!("WTF 方案未写完，原任务保留：{error}"));
    }
    Ok(())
}

fn stamp() -> String {
    format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn create_session(job: &WtfJob) -> Result<PathBuf, String> {
    let parent = job.directory.join("handoffs");
    if !parent.exists() {
        fs::create_dir(&parent).map_err(|error| error.to_string())?;
    }
    ordinary_under(&job.directory, &parent)?;
    let session = parent.join(format!("session-{}", stamp()));
    fs::create_dir(&session).map_err(|error| error.to_string())?;
    Ok(session)
}

fn prepare_handoff(job: &WtfJob, clips: &[WtfClipInput]) -> Result<(Handoff, PathBuf), String> {
    prepare_handoff_with(job, clips, import_timestamp_file)
}

fn prepare_handoff_with(
    job: &WtfJob,
    clips: &[WtfClipInput],
    importer: impl FnOnce(&Path, &Path, &Path) -> Result<serde_json::Value, String>,
) -> Result<(Handoff, PathBuf), String> {
    let plan = build_plan(job, clips)?;
    let replay = verify_source(job)?;
    let session = create_session(job)?;
    let plan_path = session.join("plan.json");
    export_new(&plan_path, &plan)?;
    let imported = importer(&replay, &plan_path, &session.join("jobs"))?;
    let recording_job_id = imported["jobId"]
        .as_str()
        .ok_or("剪辑助手没有返回录制任务编号。")?
        .to_string();
    d2_highlights_core::write_json_pretty(&session.join("import-receipt.json"), &imported)
        .map_err(|error| error.to_string())?;
    let receipt = Handoff {
        session_directory: session.display().to_string(),
        recording_job_id,
        clip_count: clips.len(),
        launched: false,
        message: "独立录制会话已保存，尚未打开剪辑助手。".into(),
    };
    d2_highlights_core::write_json_pretty(&session.join("handoff-receipt.json"), &receipt)
        .map_err(|error| error.to_string())?;
    Ok((receipt, replay))
}

fn recorder_executable() -> Result<Option<PathBuf>, String> {
    let current = env::current_exe().map_err(|error| error.to_string())?;
    let Some(directory) = current.parent() else {
        return Ok(None);
    };
    for root in std::iter::once(directory).chain(directory.parent()) {
        for name in ["猫猫的剪辑小助手.exe", "cat-cut-assistant.exe"] {
            let path = root.join(name);
            if let Ok(metadata) = fs::symlink_metadata(&path)
                && metadata.is_file()
                && !linked(&metadata)
            {
                return fs::canonicalize(path)
                    .map(Some)
                    .map_err(|error| error.to_string());
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "wtf_tests.rs"]
mod tests;
