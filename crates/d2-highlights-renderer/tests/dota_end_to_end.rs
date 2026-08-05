use d2_highlights_core::{HighlightDocument, JobManifest, TimelineDocument};
use d2_highlights_renderer::{
    BgmMode, CameraStyle, CancellationToken, ClipCameraMode, RenderClip, RenderRequest,
    RenderSettings, RenderTakeRole, render,
};
use std::path::{Path, PathBuf};

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
#[ignore = "launches an app-owned offline Dota 2 process"]
fn real_dem_exports_synchronized_camera_assets_and_closes_dota() {
    let source_job = PathBuf::from(std::env::var_os("D2H_JOB_DIR").expect("D2H_JOB_DIR"));
    let dota2_exe = PathBuf::from(std::env::var_os("DOTA2_EXE").expect("DOTA2_EXE"));
    let ffmpeg_exe = PathBuf::from(std::env::var_os("FFMPEG_EXE").expect("FFMPEG_EXE"));
    let ffprobe_exe = PathBuf::from(std::env::var_os("FFPROBE_EXE").expect("FFPROBE_EXE"));
    let manifest: JobManifest = read_json(&source_job.join("manifest.json"));
    let timeline: TimelineDocument =
        read_json(&source_job.join("timeline").join("combat-events.json"));
    let highlights: HighlightDocument =
        read_json(&source_job.join("timeline").join("highlights.json"));
    let candidate = highlights
        .candidates
        .iter()
        .find(|candidate| candidate.id == "hl-003")
        .unwrap();
    let temporary_output;
    let output_root = if let Some(path) = std::env::var_os("D2H_E2E_OUTPUT_DIR") {
        let path = PathBuf::from(path);
        std::fs::create_dir_all(&path).unwrap();
        path
    } else {
        temporary_output = tempfile::tempdir().unwrap();
        temporary_output.path().to_path_buf()
    };
    let source_start_seconds = candidate.peak_seconds - 2.5;
    let source_end_seconds = candidate.peak_seconds + 2.5;
    let take_group_id = Some("scene-e2e-01".to_string());
    let request = RenderRequest {
        job_id: "d2h-render-e2e".to_string(),
        source_sha256: manifest.source.sha256,
        job_dir: output_root,
        source_replay: PathBuf::from(manifest.source.path),
        dota2_exe,
        ffmpeg_exe,
        ffprobe_exe,
        timeline,
        clips: vec![
            RenderClip {
                clip_id: "clip-e2e-player".to_string(),
                candidate_id: candidate.id.clone(),
                view_hero: Some("npc_dota_hero_mirana".to_string()),
                camera_mode: ClipCameraMode::PlayerPerspective,
                take_group_id: take_group_id.clone(),
                take_role: RenderTakeRole::Primary,
                include_in_final: true,
                source_start_seconds,
                source_peak_seconds: candidate.peak_seconds,
                source_end_seconds,
                anchor_seconds: candidate.peak_seconds,
                anchor_tick: candidate.anchor_tick,
            },
            RenderClip {
                clip_id: "clip-e2e-close".to_string(),
                candidate_id: candidate.id.clone(),
                view_hero: Some("npc_dota_hero_mirana".to_string()),
                camera_mode: ClipCameraMode::HeroChase,
                take_group_id,
                take_role: RenderTakeRole::Alternate,
                include_in_final: false,
                source_start_seconds,
                source_peak_seconds: candidate.peak_seconds,
                source_end_seconds,
                anchor_seconds: candidate.peak_seconds,
                anchor_tick: candidate.anchor_tick,
            },
        ],
        settings: RenderSettings {
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
        },
    };
    let completed = render(request, CancellationToken::new(), |progress| {
        eprintln!(
            "{}% {}: {}",
            progress.percent, progress.stage, progress.message
        );
    })
    .unwrap();

    assert!(Path::new(&completed.output_path).is_file());
    assert!(Path::new(&completed.qc_report_path).is_file());
    assert_eq!((completed.width, completed.height), (1920, 1080));
    assert_eq!(completed.segment_count, 1);
    assert_eq!(completed.source_asset_count, 2);
    assert_eq!(completed.source_assets[0].asset_id, "S001-A");
    assert_eq!(completed.source_assets[1].asset_id, "S001-B");
    assert!(completed.source_assets[0].included_in_final);
    assert!(!completed.source_assets[1].included_in_final);
    assert!(
        Path::new(&completed.source_assets_dir)
            .join("S001-A_玩家视角.mp4")
            .is_file()
    );
    assert!(
        Path::new(&completed.source_assets_dir)
            .join("S001-B_英雄跟随.mp4")
            .is_file()
    );
    assert!(
        Path::new(&completed.source_assets_dir)
            .join("素材清单.json")
            .is_file()
    );
    assert!(completed.duration_seconds >= 5.0);
    assert!(completed.duration_seconds < 7.0);
}
