use super::*;

#[cfg(test)]
#[path = "resume_tests.rs"]
mod tests;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderResumeSummary {
    pub fingerprint: String,
    pub total_clips: usize,
    pub completed_clips: usize,
    pub remaining_clips: usize,
    pub first_pending_clip: Option<usize>,
    pub unverified_camera_clips: usize,
    pub completed_directory: Option<String>,
}

/// Read-only inspection. File completion and automatic camera identification are
/// separate facts; the UI offers an explicit action to reuse completed material.
pub fn inspect_resume(
    request: &RenderRequest,
    cancellation: &CancellationToken,
) -> Result<RenderResumeSummary, RenderError> {
    check_cancelled(cancellation)?;
    validate_request(request)?;
    let fingerprint = render_fingerprint(request)?;
    let cache_root = request
        .job_dir
        .join("render")
        .join("cache")
        .join(&fingerprint[..16]);
    let segment_root = cache_root.join("segments");
    let state_path = cache_root.join("state.json");
    if !state_path.is_file() {
        return Ok(summarize(
            request,
            fingerprint,
            &segment_root,
            None,
            &BTreeMap::new(),
        ));
    }
    let state: RenderState = serde_json::from_slice(&fs::read(state_path)?)?;
    if state.fingerprint != fingerprint || state.schema_version != RENDER_SCHEMA_VERSION {
        return Err(RenderError::InvalidPlan(
            "断点记录与当前方案不一致，请保留原记录并重新检查。".into(),
        ));
    }
    let saved = verified_segments(request, &state, &segment_root, cancellation, false)?;
    Ok(summarize(
        request,
        fingerprint,
        &segment_root,
        Some(&state),
        &saved,
    ))
}

pub(super) fn summarize(
    request: &RenderRequest,
    fingerprint: String,
    segment_root: &Path,
    state: Option<&RenderState>,
    saved: &BTreeMap<String, PathBuf>,
) -> RenderResumeSummary {
    let first_pending_clip = request
        .clips
        .iter()
        .position(|clip| !saved.contains_key(&clip.clip_id))
        .map(|index| index + 1);
    let unverified_camera_clips = request
        .clips
        .iter()
        .filter(|clip| {
            saved.contains_key(&clip.clip_id)
                && state
                    .and_then(|state| state.segments.get(&clip.clip_id))
                    .is_some_and(|segment| !segment.camera_target_verified)
        })
        .count();
    RenderResumeSummary {
        fingerprint,
        total_clips: request.clips.len(),
        completed_clips: saved.len(),
        remaining_clips: request.clips.len() - saved.len(),
        first_pending_clip,
        unverified_camera_clips,
        completed_directory: (!saved.is_empty()).then(|| segment_root.display().to_string()),
    }
}

pub(super) fn resume_requested(
    request: &RenderRequest,
    fingerprint: &str,
) -> Result<bool, RenderError> {
    match request.resume_fingerprint.as_deref() {
        None => Ok(false),
        Some(expected) if expected == fingerprint => Ok(true),
        Some(_) => Err(RenderError::InvalidPlan(
            "剪辑方案已变化，请重新打开导出窗口检查可续录片段。".into(),
        )),
    }
}

pub(super) fn verified_segments(
    request: &RenderRequest,
    state: &RenderState,
    segment_root: &Path,
    cancellation: &CancellationToken,
    recheck_quality: bool,
) -> Result<BTreeMap<String, PathBuf>, RenderError> {
    let mut saved = BTreeMap::new();
    for clip in &request.clips {
        check_cancelled(cancellation)?;
        let Some(segment) = state.segments.get(&clip.clip_id) else {
            continue;
        };
        let path = Path::new(&segment.output_path);
        if path.parent() != Some(segment_root)
            || !segment_cache_is_valid(request, clip, segment)
            || (recheck_quality
                && !run_qc(&request.ffmpeg_exe, &request.ffprobe_exe, path)
                    .is_ok_and(|qc| qc.warnings.is_empty()))
        {
            continue;
        }
        saved.insert(clip.clip_id.clone(), path.to_path_buf());
    }
    Ok(saved)
}

pub(super) fn reused_clip<F>(
    clip: &RenderClip,
    index: usize,
    total: usize,
    path: &Path,
    on_progress: &mut F,
) -> Result<PathBuf, RenderError>
where
    F: FnMut(RenderProgress),
{
    execution::record(
        "cache_hit",
        serde_json::json!({"clipId": clip.clip_id, "output": path, "reuseExplicitlyRequested": true}),
    )?;
    emit(
        on_progress,
        "capture",
        "running",
        &format!("已复用第 {}/{} 段，不重复录制", index + 1, total),
        clip_percent(index + 1, total),
        index + 1,
        total,
    );
    Ok(path.to_path_buf())
}
