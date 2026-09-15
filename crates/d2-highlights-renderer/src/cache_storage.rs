use super::*;

#[cfg(test)]
#[path = "cache_storage_tests.rs"]
mod tests;

/// Only for completed immutable media and a fresh destination. Never truncate a
/// destination: it may be a hard link to a delivered asset from an earlier run.
pub(super) fn link_or_copy_new(source: &Path, destination: &Path) -> io::Result<()> {
    execution::check_cancelled()?;
    if fs::hard_link(source, destination).is_ok() {
        return Ok(());
    }
    copy_new(source, destination)
}

fn copy_new(source: &Path, destination: &Path) -> io::Result<()> {
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = (|| {
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            execution::check_cancelled()?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
        }
        output.sync_all()
    })();
    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheCleanupSummary {
    pub fingerprint: String,
    pub file_count: usize,
    pub bytes: u64,
    pub directories: Vec<String>,
}

#[derive(Serialize)]
struct CleanupFile {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

struct CleanupPlan {
    summary: CacheCleanupSummary,
    root: PathBuf,
    files: Vec<CleanupFile>,
}

fn invalid(message: &str) -> RenderError {
    RenderError::Media(format!("缓存未清理：{message}"))
}

// An OS-held file lock also serializes recording and cleanup across application
// instances. The empty file may remain after a crash; the OS releases its lock.
pub(super) fn lock_job_storage(job_dir: &Path) -> Result<fs::File, RenderError> {
    let parent = job_dir
        .parent()
        .ok_or_else(|| invalid("任务根目录无效。"))?;
    regular_path(parent, job_dir, true)?;
    let render = job_dir.join("render");
    if !render.exists() {
        fs::create_dir(&render)?;
    }
    regular_path(job_dir, &render, true)?;
    let path = render.join("storage.lock");
    if path.exists() {
        regular_path(job_dir, &path, false)?;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock()
        .map_err(|_| invalid("当前任务正在录制或清理，请等待该操作结束。"))?;
    Ok(file)
}

// Reject junctions and all reparse points as well as symbolic links. Check each
// component, not only the leaf, before following any stored path.
fn regular_path(root: &Path, path: &Path, directory: bool) -> Result<PathBuf, RenderError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid("路径不在当前任务内。"))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(invalid("路径包含非普通目录分量。"));
        }
        current.push(component);
        let metadata = fs::symlink_metadata(&current)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(invalid("不清理链接或重解析目录。"));
            }
        }
        if metadata.file_type().is_symlink() {
            return Err(invalid("不清理符号链接。"));
        }
    }
    let metadata = fs::metadata(&current)?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(invalid("路径类型已变化。"));
    }
    Ok(current)
}

pub fn inspect_render_cache(
    job_dir: &Path,
    report_path: &Path,
    ffprobe: &Path,
    cancellation: &CancellationToken,
) -> Result<CacheCleanupSummary, RenderError> {
    Ok(plan_cleanup(job_dir, report_path, ffprobe, cancellation)?.summary)
}

/// Caller holds the same exclusive activity lease used by recording, saving,
/// and inspection. The client supplies only a snapshot digest, never file names.
pub fn clean_render_cache(
    job_dir: &Path,
    report_path: &Path,
    ffprobe: &Path,
    expected: &str,
    cancellation: &CancellationToken,
) -> Result<CacheCleanupSummary, RenderError> {
    let _storage_lease = lock_job_storage(job_dir)?;
    let plan = plan_cleanup(job_dir, report_path, ffprobe, cancellation)?;
    apply_cleanup(plan, expected, cancellation)
}

fn apply_cleanup(
    plan: CleanupPlan,
    expected: &str,
    cancellation: &CancellationToken,
) -> Result<CacheCleanupSummary, RenderError> {
    if plan.summary.fingerprint != expected {
        return Err(invalid("缓存或交付文件已变化，请重新检查并确认清理范围。"));
    }
    for (removed, file) in plan.files.iter().enumerate() {
        check_cancelled(cancellation).map_err(|_| {
            invalid(&format!(
                "已清理 {removed} 个文件，操作已取消；其余文件保留。"
            ))
        })?;
        let result = (|| {
            regular_path(&plan.root, &file.path, false)?;
            let metadata = fs::metadata(&file.path)?;
            if metadata.len() != file.bytes || metadata.modified()? != file.modified {
                return Err(invalid("原始文件在检查后发生变化。"));
            }
            fs::remove_file(&file.path)?;
            Ok::<_, RenderError>(())
        })();
        if let Err(error) = result {
            return Err(invalid(&format!(
                "已清理 {removed} 个文件，剩余文件保留。{error}"
            )));
        }
    }
    Ok(plan.summary)
}

fn plan_cleanup(
    job_dir: &Path,
    report_path: &Path,
    ffprobe: &Path,
    cancellation: &CancellationToken,
) -> Result<CleanupPlan, RenderError> {
    plan_cleanup_with(job_dir, report_path, cancellation, &mut |path, duration| {
        validate_segment_media(&probe_media(ffprobe, path)?, duration)
    })
}

fn plan_cleanup_with(
    job_dir: &Path,
    report_path: &Path,
    cancellation: &CancellationToken,
    verify_media: &mut impl FnMut(&Path, f32) -> Result<(), RenderError>,
) -> Result<CleanupPlan, RenderError> {
    check_cancelled(cancellation)?;
    // Use the supplied absolute job path for stored pre-1.9.15 paths as well.
    // Reject a junction job itself before inspecting any data beneath it.
    let parent = job_dir
        .parent()
        .ok_or_else(|| invalid("任务根目录无效。"))?;
    regular_path(parent, job_dir, true)?;
    let root = job_dir.to_path_buf();
    let output = root.join("output");
    if report_path.parent() != Some(output.as_path())
        || !report_path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.ends_with("_质量报告.json"))
    {
        return Err(invalid("只能检查当前任务的成片质量报告。"));
    }
    regular_path(&root, report_path, false)?;
    let report_bytes = fs::read(report_path)?;
    let report: QcReport = serde_json::from_slice(&report_bytes)?;
    let report_stem = report_path.file_name().unwrap().to_string_lossy();
    let stem = report_stem.strip_suffix("_质量报告.json").unwrap();
    let final_path = output.join(format!("{stem}.mp4"));
    let assets_dir = output.join(format!("{stem}_源素材"));
    regular_path(&root, &final_path, false)?;
    regular_path(&root, &assets_dir, true)?;
    if report.schema_version != RENDER_SCHEMA_VERSION
        || !report.has_video
        || !report.has_audio
        || report.source_assets.is_empty()
        || report.final_segment_count == 0
    {
        return Err(invalid("缺少完整的已完成交付记录。"));
    }
    verify_media(&final_path, report.duration_seconds)?;
    let mut digest = Sha256::new();
    digest.update(&report_bytes);
    digest.update(file_sha256(&final_path)?);
    let mut asset_hashes = BTreeMap::new();
    for asset in &report.source_assets {
        check_cancelled(cancellation)?;
        let path = Path::new(&asset.output_path);
        if path.parent() != Some(assets_dir.as_path()) {
            return Err(invalid("源素材路径不匹配。"));
        }
        regular_path(&root, path, false)?;
        let hash = file_sha256(path)?;
        if asset_hashes
            .insert(asset.clip_id.clone(), (path.to_path_buf(), hash.clone()))
            .is_some()
        {
            return Err(invalid("源素材编号重复。"));
        }
        digest.update(hash);
    }
    let cache_dir = root.join("render").join("cache");
    regular_path(&root, &cache_dir, true)?;
    let mut files = Vec::new();
    let mut directories = Vec::new();
    let mut matched = false;
    let mut caches = fs::read_dir(&cache_dir)?.collect::<Result<Vec<_>, _>>()?;
    caches.sort_by_key(|entry| entry.path());
    for entry in caches {
        check_cancelled(cancellation)?;
        let cache = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        regular_path(&root, &cache, true)?;
        let state_path = cache.join("state.json");
        if !state_path.exists() {
            continue;
        }
        regular_path(&root, &state_path, false)?;
        let state_bytes = fs::read(&state_path)?;
        let Ok(state) = serde_json::from_slice::<RenderState>(&state_bytes) else {
            continue;
        };
        if state.schema_version != RENDER_SCHEMA_VERSION
            || state.segments.len() != asset_hashes.len()
            || !state.segments.iter().all(|(id, seg)| {
                asset_hashes.get(id).is_some_and(|(_, hash)| {
                    hash == &seg.content_sha256
                        && !hash.is_empty()
                        && matches!(
                            seg.status.as_str(),
                            "complete" | "quality_warning" | "camera_target_unverified"
                        )
                })
            })
        {
            continue;
        }
        let segment_root = cache.join("segments");
        regular_path(&root, &segment_root, true)?;
        let mut pending_paths = HashSet::new();
        for (id, seg) in &state.segments {
            check_cancelled(cancellation)?;
            let path = Path::new(&seg.output_path);
            if path.parent() != Some(segment_root.as_path()) {
                return Err(invalid("缓存片段路径不匹配。"));
            }
            regular_path(&root, path, false)?;
            if file_sha256(path)? != seg.content_sha256 {
                return Err(invalid("缓存 MP4 与交付记录不一致。"));
            }
            verify_media(&asset_hashes[id].0, seg.duration_seconds)?;
            pending_paths.insert(path.with_extension("partial.mp4"));
        }
        matched = true;
        digest.update(&state_bytes);
        for clip in fs::read_dir(&segment_root)? {
            let clip = clip?;
            if !clip.file_type()?.is_dir() {
                continue;
            }
            let clip_dir = regular_path(&root, &clip.path(), true)?;
            let quality_path = clip_dir.join("quality.json");
            if !quality_path.exists() {
                continue;
            }
            regular_path(&root, &quality_path, false)?;
            let quality_bytes = fs::read(&quality_path)?;
            let Ok(quality) = serde_json::from_slice::<QcReport>(&quality_bytes) else {
                continue;
            };
            if !pending_paths.contains(Path::new(&quality.output_path))
                || !quality.has_video
                || !quality.has_audio
            {
                continue;
            }
            let raw = clip_dir.join("raw");
            if !raw.exists() {
                continue;
            }
            regular_path(&root, &raw, true)?;
            let before = files.len();
            for entry in fs::read_dir(&raw)? {
                check_cancelled(cancellation)?;
                let entry = entry?;
                if !raw_capture_name(&entry.file_name().to_string_lossy()) {
                    continue;
                }
                let path = regular_path(&root, &entry.path(), false)?;
                let metadata = fs::metadata(&path)?;
                files.push(CleanupFile {
                    path,
                    bytes: metadata.len(),
                    modified: metadata.modified()?,
                });
            }
            if files.len() > before {
                directories.push(raw.display().to_string());
            }
        }
    }
    if !matched {
        return Err(invalid(
            "没有与全部交付素材一致的完整缓存；失败和未完成任务会保留。",
        ));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    directories.sort();
    digest.update(serde_json::to_vec(&files)?);
    let summary = CacheCleanupSummary {
        fingerprint: format!("{:x}", digest.finalize()),
        file_count: files.len(),
        bytes: files.iter().map(|f| f.bytes).sum(),
        directories,
    };
    Ok(CleanupPlan {
        summary,
        root,
        files,
    })
}

fn raw_capture_name(name: &str) -> bool {
    name == "game.wav"
        || name
            .strip_prefix("frame-")
            .and_then(|v| v.strip_suffix(".jpg"))
            .is_some_and(|digits| digits.len() >= 6 && digits.bytes().all(|c| c.is_ascii_digit()))
}
