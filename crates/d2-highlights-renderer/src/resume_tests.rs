use super::*;
use d2_highlights_core::{ParserIdentity, ReplayMetadata, ReplayPlayer};

fn request(root: &Path) -> RenderRequest {
    RenderRequest {
        job_id: "resume-fixture".into(),
        source_sha256: "fixture".into(),
        job_dir: root.into(),
        source_replay: root.join("fixture.dem"),
        dota2_exe: root.join("must-never-launch.exe"),
        ffmpeg_exe: PathBuf::new(),
        ffprobe_exe: PathBuf::new(),
        timeline: TimelineDocument {
            schema_version: "1.1".into(),
            source_sha256: "fixture".into(),
            parser: ParserIdentity {
                name: "fixture".into(),
                version: "1".into(),
            },
            replay: ReplayMetadata {
                playback_ticks: 1800,
                playback_time_seconds: 60.0,
                game_build: 42,
                match_id: None,
                game_mode: None,
                game_winner: None,
                players: vec![ReplayPlayer {
                    slot: 3,
                    hero_name: "npc_dota_hero_mirana".into(),
                    player_name: None,
                    game_team: Some(2),
                    is_fake_client: false,
                }],
            },
            events: vec![],
            tree_orders: vec![],
            temporary_trees: vec![],
            salutes: vec![],
            state_snapshots: vec![],
        },
        clips: (1..=3)
            .map(|n| RenderClip {
                clip_id: format!("clip-{n}"),
                candidate_id: format!("event-{n}"),
                view_hero: Some("npc_dota_hero_mirana".into()),
                camera_mode: ClipCameraMode::PlayerPerspective,
                take_group_id: None,
                take_role: RenderTakeRole::Primary,
                include_in_final: true,
                source_start_seconds: 10.0,
                source_peak_seconds: 12.0,
                source_end_seconds: 14.0,
                anchor_seconds: 12.0,
                anchor_tick: 360,
            })
            .collect(),
        settings: RenderSettings::default(),
        resume_fingerprint: None,
    }
}

#[test]
fn resume_requires_the_inspected_plan_and_does_not_change_media_identity() {
    let mut request = request(Path::new("fixture"));
    let original = render_fingerprint(&request).unwrap();
    assert!(!resume_requested(&request, &original).unwrap());
    request.resume_fingerprint = Some(original.clone());
    assert!(resume_requested(&request, &original).unwrap());
    assert_eq!(render_fingerprint(&request).unwrap(), original);
    request.clips[0].source_start_seconds = 9.0;
    let changed = render_fingerprint(&request).unwrap();
    assert_ne!(changed, original);
    assert!(resume_requested(&request, &changed).is_err());
}

#[test]
fn summary_uses_original_order_and_handles_holes_or_all_saved() {
    let request = request(Path::new("fixture"));
    let mut saved = BTreeMap::new();
    for id in ["clip-1", "clip-3"] {
        saved.insert(id.to_string(), PathBuf::from(id));
    }
    let partial = summarize(&request, "plan".into(), Path::new("segments"), None, &saved);
    assert_eq!(
        (
            partial.completed_clips,
            partial.remaining_clips,
            partial.first_pending_clip
        ),
        (2, 1, Some(2))
    );
    saved.insert("clip-2".into(), PathBuf::from("clip-2"));
    let complete = summarize(&request, "plan".into(), Path::new("segments"), None, &saved);
    assert_eq!(
        (
            complete.completed_clips,
            complete.remaining_clips,
            complete.first_pending_clip
        ),
        (3, 0, None)
    );
}

#[test]
fn missing_checkpoint_inspection_is_read_only_and_cancelled_work_stops() {
    let temp = tempfile::tempdir().unwrap();
    let mut request = request(temp.path());
    fs::write(&request.source_replay, b"isolated replay fixture").unwrap();
    fs::write(&request.dota2_exe, b"not executable").unwrap();
    request.source_sha256 = file_sha256(&request.source_replay).unwrap();
    request.timeline.source_sha256 = request.source_sha256.clone();
    assert_eq!(
        inspect_resume(&request, &CancellationToken::new())
            .unwrap()
            .completed_clips,
        0
    );
    assert!(!temp.path().join("render").exists());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        inspect_resume(&request, &cancelled),
        Err(RenderError::Cancelled)
    ));
}

#[test]
#[ignore = "requires local FFmpeg; synthetic checkpoint files only, never launches Dota"]
fn synthetic_interrupted_capture_resumes_and_composes_without_game_launch() {
    let temp = tempfile::tempdir().unwrap();
    let mut request = request(temp.path());
    fs::write(&request.source_replay, b"isolated replay fixture").unwrap();
    fs::write(&request.dota2_exe, b"must never be executed").unwrap();
    request.source_sha256 = file_sha256(&request.source_replay).unwrap();
    request.timeline.source_sha256 = request.source_sha256.clone();
    request.ffmpeg_exe = std::env::var_os("D2H_FFMPEG").expect("D2H_FFMPEG").into();
    request.ffprobe_exe = std::env::var_os("D2H_FFPROBE").expect("D2H_FFPROBE").into();
    let fingerprint = render_fingerprint(&request).unwrap();
    let cache = temp.path().join("render/cache").join(&fingerprint[..16]);
    let segments = cache.join("segments");
    fs::create_dir_all(&segments).unwrap();
    let media_path = segments.join("synthetic.mp4");
    let output = background_command(&request.ffmpeg_exe)
        .args([
            "-hide_banner",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=60:duration=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=4",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-ac",
            "2",
            "-shortest",
            &path_arg(&media_path),
        ])
        .output()
        .unwrap();
    require_success(&output, "synthetic fixture").unwrap();
    assert!(
        run_qc(&request.ffmpeg_exe, &request.ffprobe_exe, &media_path)
            .unwrap()
            .warnings
            .is_empty()
    );
    let hash = file_sha256(&media_path).unwrap();
    let mut state = RenderState {
        schema_version: RENDER_SCHEMA_VERSION.into(),
        fingerprint: fingerprint.clone(),
        updated_unix_seconds: 1,
        segments: BTreeMap::new(),
    };
    for clip in &request.clips {
        state.segments.insert(
            clip.clip_id.clone(),
            SegmentState {
                candidate_id: clip.candidate_id.clone(),
                output_path: path_arg(&media_path),
                duration_seconds: 4.0,
                status: "camera_target_unverified".into(),
                capture_pipeline: CAPTURE_PIPELINE_VERSION.into(),
                content_sha256: hash.clone(),
                quality_checked: true,
                camera_target_verified: false,
                expected_duration_seconds: 4.0,
            },
        );
    }
    let third = state.segments.remove("clip-3").unwrap();
    let state_path = cache.join("state.json");
    write_json(&state_path, &state).unwrap();
    let state_before = file_sha256(&state_path).unwrap();
    let inspected = inspect_resume(&request, &CancellationToken::new()).unwrap();
    assert_eq!(
        (
            inspected.completed_clips,
            inspected.remaining_clips,
            inspected.first_pending_clip,
            inspected.unverified_camera_clips
        ),
        (2, 1, Some(3), 2)
    );
    assert_eq!(file_sha256(&state_path).unwrap(), state_before);
    state.segments.get_mut("clip-2").unwrap().content_sha256 = "0".repeat(64);
    write_json(&state_path, &state).unwrap();
    assert_eq!(
        inspect_resume(&request, &CancellationToken::new())
            .unwrap()
            .completed_clips,
        1
    );
    state.segments.get_mut("clip-2").unwrap().content_sha256 = hash.clone();
    state.segments.get_mut("clip-2").unwrap().output_path = path_arg(&segments.join("missing.mp4"));
    write_json(&state_path, &state).unwrap();
    assert_eq!(
        inspect_resume(&request, &CancellationToken::new())
            .unwrap()
            .completed_clips,
        1
    );
    state.segments.get_mut("clip-2").unwrap().output_path = path_arg(&media_path);
    state.segments.insert("clip-3".into(), third);
    write_json(&state_path, &state).unwrap();
    let mut changed = request.clone();
    changed.settings.clean_hud = !changed.settings.clean_hud;
    assert_eq!(
        inspect_resume(&changed, &CancellationToken::new())
            .unwrap()
            .completed_clips,
        0
    );
    request.resume_fingerprint = Some(fingerprint);
    let completed = render(request, CancellationToken::new(), |_| {}).unwrap();
    assert_eq!(
        (completed.segment_count, completed.source_asset_count),
        (3, 3)
    );
    assert!((completed.duration_seconds - 12.0).abs() < 0.2);
    assert_eq!(
        completed
            .source_assets
            .iter()
            .map(|a| a.asset_id.as_str())
            .collect::<Vec<_>>(),
        ["S001-A", "S002-A", "S003-A"]
    );
    assert_eq!(file_sha256(&media_path).unwrap(), hash);
    assert!(
        !state
            .segments
            .values()
            .any(|segment| segment.camera_target_verified)
    );
    let logs = fs::read_dir(temp.path().join("render/logs")).unwrap();
    let log = fs::read_to_string(logs.into_iter().next().unwrap().unwrap().path()).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("\"event\":\"cache_hit\""))
            .count(),
        3
    );
    for forbidden in [
        "\"event\":\"capture_begin\"",
        "\"event\":\"dota_started\"",
        "startmovie",
    ] {
        assert!(!log.contains(forbidden));
    }
}
