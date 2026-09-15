use super::*;
use d2_highlights_renderer::CacheCleanupSummary;

#[tauri::command]
pub(crate) async fn inspect_render_cache(
    job_id: String,
    report_name: String,
    state: State<'_, AppState>,
) -> Result<CacheCleanupSummary, String> {
    run(job_id, report_name, None, state).await
}

#[tauri::command]
pub(crate) async fn clean_render_cache(
    job_id: String,
    report_name: String,
    fingerprint: String,
    state: State<'_, AppState>,
) -> Result<CacheCleanupSummary, String> {
    run(job_id, report_name, Some(fingerprint), state).await
}

async fn run(
    job_id: String,
    report_name: String,
    fingerprint: Option<String>,
    state: State<'_, AppState>,
) -> Result<CacheCleanupSummary, String> {
    if !valid_job_id(&job_id)
        || report_name.contains(['/', '\\', ':'])
        || !report_name.ends_with("_质量报告.json")
    {
        return Err("任务编号或质量报告名称无效。".into());
    }
    let lease = state.activities.begin(Activity::Render)?;
    let cancellation = register_render_cancellation(&state.activities, &state.render_runtime)?;
    let worker = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let _runtime_guard = RenderRuntimeGuard(worker.render_runtime.clone());
        let job_dir = worker.jobs_root.join(job_id);
        let report = job_dir.join("output").join(report_name);
        let ffprobe = worker
            .ffprobe_exe
            .as_deref()
            .ok_or("找不到 FFprobe，无法验证交付视频。")?;
        match fingerprint {
            Some(expected) => d2_highlights_renderer::clean_render_cache(
                &job_dir,
                &report,
                ffprobe,
                &expected,
                &cancellation,
            ),
            None => d2_highlights_renderer::inspect_render_cache(
                &job_dir,
                &report,
                ffprobe,
                &cancellation,
            ),
        }
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
