use super::*;
use serde_json::json;

struct Fixture {
    temp: tempfile::TempDir,
    report: PathBuf,
    raw: PathBuf,
    segment: PathBuf,
    asset: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let output = root.join("output");
        let assets = output.join("match_源素材");
        let cache = root.join("render/cache/test-cache");
        let segment = cache.join("segments/01-clip-1-100.mp4");
        let clip_dir = cache.join("segments/01-clip-1-101");
        let raw = clip_dir.join("raw");
        fs::create_dir_all(&raw).unwrap();
        fs::create_dir_all(&assets).unwrap();
        let asset = assets.join("S001-A_玩家视角.mp4");
        fs::write(&segment, b"complete synthetic video").unwrap();
        link_or_copy_new(&segment, &asset).unwrap();
        fs::write(output.join("match.mp4"), b"complete synthetic final").unwrap();
        fs::write(clip_dir.join("base.mp4"), b"keep base").unwrap();
        fs::write(raw.join("frame-000001.jpg"), b"frame").unwrap();
        fs::write(raw.join("game.wav"), b"audio").unwrap();
        fs::write(raw.join("notes.txt"), b"keep unknown").unwrap();
        let hash = file_sha256(&segment).unwrap();
        let state = cache.join("state.json");
        fs::write(&state, serde_json::to_vec(&json!({
            "schema_version": RENDER_SCHEMA_VERSION, "fingerprint": "fixture", "updated_unix_seconds": 1,
            "segments": {"clip-1": {
                "candidate_id": "candidate", "output_path": segment, "duration_seconds": 1,
                "status": "complete", "content_sha256": hash
            }}
        })).unwrap()).unwrap();
        let mut qc = json!({
            "schema_version": RENDER_SCHEMA_VERSION, "output_path": output.join("match.mp4"),
            "duration_seconds": 1, "width": 1920, "height": 1080, "has_video": true, "has_audio": true,
            "black_events": 0, "freeze_events": 0, "audio_mean_db": -20, "audio_peak_db": -5,
            "final_segment_count": 1, "source_assets_dir": assets, "warnings": [],
            "source_assets": [{"assetId":"S001-A", "sceneNumber":1, "takeIndex":0, "clipId":"clip-1",
                "takeGroupId":null, "takeRole":"primary", "includedInFinal":true, "outputPath":asset,
                "viewHero":null, "cameraMode":"player_perspective", "sourceStartSeconds":0, "sourceEndSeconds":1}]
        });
        let report = output.join("match_质量报告.json");
        fs::write(&report, serde_json::to_vec(&qc).unwrap()).unwrap();
        qc["output_path"] = json!(segment.with_extension("partial.mp4"));
        fs::write(
            clip_dir.join("quality.json"),
            serde_json::to_vec(&qc).unwrap(),
        )
        .unwrap();
        Self {
            temp,
            report,
            raw,
            segment,
            asset,
            state,
        }
    }

    fn plan(&self) -> Result<CleanupPlan, RenderError> {
        let mut checked = 0;
        let result = plan_cleanup_with(
            self.temp.path(),
            &self.report,
            &CancellationToken::new(),
            &mut |path, duration| {
                assert!(path.is_file());
                assert_eq!(duration, 1.0);
                checked += 1;
                Ok(())
            },
        );
        if result.is_ok() {
            assert_eq!(checked, 2);
        }
        result
    }
}

#[test]
fn links_share_storage_and_never_overwrite_existing_media() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.mp4");
    let asset = temp.path().join("asset.mp4");
    fs::write(&source, b"original").unwrap();
    link_or_copy_new(&source, &asset).unwrap();
    // Shared identity is demonstrated through a fixture-only mutation.
    fs::write(&source, b"shared").unwrap();
    assert_eq!(fs::read(&asset).unwrap(), b"shared");
    assert!(link_or_copy_new(&source, &asset).is_err());
    assert!(copy_new(&source, &asset).is_err());
    assert_eq!(fs::read(&asset).unwrap(), b"shared");
    fs::remove_file(&source).unwrap();
    assert_eq!(fs::read(&asset).unwrap(), b"shared");
}

#[test]
fn fallback_copy_has_independent_content_and_preserves_destination() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.mp4");
    let target = temp.path().join("target.mp4");
    fs::write(&source, b"original").unwrap();
    copy_new(&source, &target).unwrap();
    fs::write(&source, b"changed").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"original");
    assert!(link_or_copy_new(&source, &target).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"original");
}

#[test]
fn cleanup_requires_confirmation_and_preserves_all_media_and_unknown_files() {
    let f = Fixture::new();
    let preview = f.plan().unwrap().summary;
    assert_eq!(preview.file_count, 2);
    assert_eq!(preview.bytes, 10);
    assert!(f.raw.join("frame-000001.jpg").exists());
    assert!(apply_cleanup(f.plan().unwrap(), "stale", &CancellationToken::new()).is_err());
    assert!(f.raw.join("game.wav").exists());
    apply_cleanup(
        f.plan().unwrap(),
        &preview.fingerprint,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(!f.raw.join("game.wav").exists());
    assert!(f.raw.join("notes.txt").exists());
    assert!(f.segment.exists() && f.asset.exists() && f.state.exists());
    assert!(f.raw.parent().unwrap().join("base.mp4").exists());
    assert_eq!(f.plan().unwrap().summary.file_count, 0);
}

#[test]
fn stale_preview_missing_asset_and_incomplete_cache_cannot_delete() {
    let f = Fixture::new();
    let preview = f.plan().unwrap().summary;
    fs::write(f.raw.join("frame-000002.jpg"), b"new").unwrap();
    assert!(
        apply_cleanup(
            f.plan().unwrap(),
            &preview.fingerprint,
            &CancellationToken::new()
        )
        .is_err()
    );
    assert!(f.raw.join("game.wav").exists());
    fs::remove_file(&f.asset).unwrap();
    assert!(f.plan().is_err());
    link_or_copy_new(&f.segment, &f.asset).unwrap();
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&f.state).unwrap()).unwrap();
    state["segments"]["clip-1"]["status"] = json!("failed");
    fs::write(&f.state, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(f.plan().is_err());
    assert!(f.raw.join("game.wav").exists());
}

#[test]
fn corrupt_media_and_cancelled_cleanup_preserve_raw_files() {
    let f = Fixture::new();
    let result = plan_cleanup_with(
        f.temp.path(),
        &f.report,
        &CancellationToken::new(),
        &mut |_, _| Err(invalid("bad media")),
    );
    assert!(result.is_err());
    let plan = f.plan().unwrap();
    let expected = plan.summary.fingerprint.clone();
    let token = CancellationToken::new();
    token.cancel();
    assert!(apply_cleanup(plan, &expected, &token).is_err());
    fs::write(&f.segment, b"corrupted").unwrap();
    assert!(f.plan().is_err());
    assert!(f.raw.join("game.wav").exists());
}

#[test]
fn path_escape_and_unmatched_attempt_are_preserved() {
    let f = Fixture::new();
    assert!(regular_path(f.temp.path(), &f.temp.path().join("../outside"), false).is_err());
    let stale = f
        .raw
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("old-attempt/raw");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("frame-000001.jpg"), b"unfinished").unwrap();
    let plan = f.plan().unwrap();
    let expected = plan.summary.fingerprint.clone();
    apply_cleanup(plan, &expected, &CancellationToken::new()).unwrap();
    assert!(stale.join("frame-000001.jpg").exists());
}

#[test]
fn raw_name_allowlist_excludes_unrelated_files() {
    for name in ["frame-000001.jpg", "game.wav"] {
        assert!(raw_capture_name(name));
    }
    for name in [
        "frame-notes.jpg",
        "frame-001.jpg",
        "other.wav",
        "base.mp4",
        "../game.wav",
    ] {
        assert!(!raw_capture_name(name));
    }
}

#[test]
fn storage_lease_blocks_concurrent_recording_and_cleanup_and_releases_on_drop() {
    let f = Fixture::new();
    let lease = lock_job_storage(f.temp.path()).unwrap();
    assert!(lock_job_storage(f.temp.path()).is_err());
    assert!(
        clean_render_cache(
            f.temp.path(),
            &f.report,
            Path::new("unused"),
            "unused",
            &CancellationToken::new()
        )
        .is_err()
    );
    assert!(f.raw.join("game.wav").exists());
    drop(lease);
    assert!(lock_job_storage(f.temp.path()).is_ok());
}

#[cfg(windows)]
#[test]
fn junction_in_cleanup_tree_is_rejected_without_touching_its_target() {
    let f = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("game.wav"), b"keep").unwrap();
    let link = f.raw.join("frame-000002.jpg");
    let result = background_command("cmd.exe")
        .args(["/C", "mklink", "/J"])
        .arg(link.to_string_lossy().replace('/', "\\"))
        .arg(outside.path().to_string_lossy().replace('/', "\\"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let rejected = f.plan().is_err();
    let kept = fs::read(outside.path().join("game.wav")).unwrap();
    // Remove the junction entry itself, never recurse through its target.
    fs::remove_dir(&link).unwrap();
    assert!(rejected);
    assert_eq!(kept, b"keep");
}

#[test]
fn completed_legacy_unverified_camera_media_is_cleanable_without_changing_state() {
    let f = Fixture::new();
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&f.state).unwrap()).unwrap();
    state["segments"]["clip-1"]["status"] = json!("camera_target_unverified");
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(&f.state, &bytes).unwrap();
    let plan = f.plan().unwrap();
    assert_eq!(plan.summary.file_count, 2);
    let expected = plan.summary.fingerprint.clone();
    apply_cleanup(plan, &expected, &CancellationToken::new()).unwrap();
    assert_eq!(fs::read(&f.state).unwrap(), bytes);
}

#[test]
#[ignore = "Explicit read-only check against a supplied job and FFprobe; never cleans or launches Dota"]
fn inspect_supplied_job_read_only() {
    let job = PathBuf::from(std::env::var("D2H_CACHE_CHECK_JOB").unwrap());
    let report = PathBuf::from(std::env::var("D2H_CACHE_CHECK_REPORT").unwrap());
    let probe = PathBuf::from(std::env::var("D2H_CACHE_CHECK_FFPROBE").unwrap());
    let summary = inspect_render_cache(&job, &report, &probe, &CancellationToken::new()).unwrap();
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
}
