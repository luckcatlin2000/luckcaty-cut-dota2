// Read-only integration proof using the saved highlight plan.
use super::*;

#[test]
#[ignore = "requires explicitly selected existing job and local tools; reads only, never launches Dota"]
fn inspect_existing_recording_checkpoint_read_only() {
    let job_dir = PathBuf::from(std::env::var_os("D2H_RESUME_JOB_DIR").expect("job folder"));
    let job_id = job_dir.file_name().unwrap().to_str().unwrap();
    let logs = job_dir.join("render/logs");
    let before_logs = fs::read_dir(&logs).unwrap().count();
    let state = AppState {
        jobs_root: job_dir.parent().unwrap().into(),
        dota2_runtime: Arc::new(Mutex::new(Dota2Runtime {
            executable: Some(
                std::env::var_os("D2H_DOTA2_EXE")
                    .expect("Dota path, read only")
                    .into(),
            ),
            source: Dota2PathSource::Custom,
        })),
        dota2_settings_path: PathBuf::new(),
        video_output: video_output::Settings::default(),
        ffmpeg_exe: Some(std::env::var_os("D2H_FFMPEG").expect("FFmpeg").into()),
        ffprobe_exe: Some(std::env::var_os("D2H_FFPROBE").expect("FFprobe").into()),
        render_runtime: Arc::new(Mutex::new(RenderRuntime::default())),
        activities: Activities::default(),
    };
    let request = build_render_request(job_id, &state).unwrap();
    let summary = inspect_resume(&request, &CancellationToken::new()).unwrap();
    println!(
        "READ_ONLY_RESUME {}",
        serde_json::to_string(&summary).unwrap()
    );
    let expected: usize = std::env::var("D2H_RESUME_EXPECTED_COMPLETED")
        .expect("expected completed count")
        .parse()
        .unwrap();
    assert_eq!(summary.completed_clips, expected);
    assert_eq!(summary.total_clips, request.clips.len());
    assert_eq!(before_logs, fs::read_dir(logs).unwrap().count());
}
