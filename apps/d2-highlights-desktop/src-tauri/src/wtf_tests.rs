use super::*;
use serde_json::{Value, json};

const JOB: &str = "d2h-0123456789abcdef";
const HERO: &str = "npc_dota_hero_mirana";
const OTHER: &str = "npc_dota_hero_nevermore";

fn fixture(root: &Path) -> WtfJob {
    let dem = root.join("fixture.dem");
    fs::write(&dem, b"PBDEMS2\0\0\0\0\0\0\0\0\0").unwrap();
    let source = d2_highlights_core::inspect_dem(&dem).unwrap();
    let directory = root.join("wtf-jobs").join(JOB);
    let metadata = json!({"playback_ticks":9000,"playback_time_seconds":300,"game_build":1,"match_id":42,
        "players":[{"slot":1,"hero_name":HERO,"is_fake_client":false},{"slot":2,"hero_name":OTHER,"is_fake_client":false}]});
    let timeline: TimelineDocument = serde_json::from_value(json!({"schema_version":"1.4","source_sha256":source.sha256,
        "parser":{"name":"fixture","version":"1"},"replay":metadata,
        "events":[{"tick":3000,"time_seconds":100,"event_type":"DotaCombatlogDeath","attacker":HERO,"target":OTHER,"assist_players":[]}]})).unwrap();
    let highlights: HighlightDocument = serde_json::from_value(json!({"schema_version":"1.4","source_sha256":source.sha256,"detector":{"name":"rules-v2","version":"0.6.1"},
        "candidates":[{"id":"hk-001","rank":1,"kind":"hero_kill_sequence","title":"通用击杀片段","score":1,"start_seconds":90,"peak_seconds":100,"end_seconds":110,"hero_deaths":1,"anchor_tick":3000,"primary_hero":HERO,"participants":[HERO,OTHER],"reasons":[]}]})).unwrap();
    let shots = json!([
        {"id":"shot-player","order":1,"take_group_id":"take-001","take_role":"primary","include_in_default_cut":true,"beat_id":"beat-001","candidate_id":"hk-001","source_start_seconds":90,"source_end_seconds":110,"camera_mode":"player_perspective","target_hero":HERO,"framing":"normal_gameplay","purpose":"正常操作视角","fallback":{"camera_mode":"player_perspective","target_hero":HERO,"reason":"保持玩家视角"}},
        {"id":"shot-chase","order":2,"take_group_id":"take-001","take_role":"alternate","include_in_default_cut":false,"beat_id":"beat-001","candidate_id":"hk-001","source_start_seconds":90,"source_end_seconds":110,"camera_mode":"hero_chase","target_hero":OTHER,"framing":"close_action","purpose":"目标反应备用机位","fallback":{"camera_mode":"player_perspective","target_hero":HERO,"reason":"无法跟随时回到玩家"}}
    ]);
    let story = json!({"id":"story-001","rank":1,"category":"skill","template_id":"hero_kill_sequence","title":"真实来源故事","primary_hero":HERO,"participants":[],"candidate_ids":["hk-001"],"source_start_seconds":90,"source_peak_seconds":100,"source_end_seconds":110,"priority_score":1,"confidence":{"level":"high","score":1,"reasons":[]},"review_required":true,"evidence":[],"beats":[],"shots":shots,"switch_windows":[],"reasons":[]});
    let stories: StoryDocument = serde_json::from_value(json!({"schema_version":"1.2","source_sha256":source.sha256,"detector":{"name":"rules-v2","version":"0.6.1"},"stories":[story]})).unwrap();
    let manifest = JobManifest {
        schema_version: "1.0".into(),
        job_id: JOB.into(),
        created_unix_seconds: 0,
        source,
        stages: Default::default(),
    };
    for (path, value) in [
        ("manifest.json", serde_json::to_value(&manifest).unwrap()),
        (
            "timeline/combat-events.json",
            serde_json::to_value(&timeline).unwrap(),
        ),
        (
            "timeline/highlights.json",
            serde_json::to_value(&highlights).unwrap(),
        ),
        (
            "director/stories.json",
            serde_json::to_value(&stories).unwrap(),
        ),
    ] {
        d2_highlights_core::write_json_pretty(&directory.join(path), &value).unwrap();
    }
    WtfJob {
        directory,
        manifest,
        timeline,
        highlights,
        stories,
    }
}

fn clips() -> Vec<WtfClipInput> {
    serde_json::from_value(json!([
        {"clipId":"clip-player","storyId":"story-001","shotId":"shot-player","hero":HERO,"startSeconds":90,"endSeconds":110,"cameraMode":"player_perspective","takeGroupId":"wtf-story-001-take-001","takeRole":"primary","includeInFinal":true},
        {"clipId":"clip-chase","storyId":"story-001","shotId":"shot-chase","hero":OTHER,"startSeconds":90,"endSeconds":110,"cameraMode":"hero_chase","takeGroupId":"wtf-story-001-take-001","takeRole":"alternate","includeInFinal":false}
    ])).unwrap()
}

#[test]
fn only_independent_wtf_mode_is_allowed_to_use_commands() {
    assert!(require_wtf(&DesktopModule::Wtf).is_ok());
    assert!(require_wtf(&DesktopModule::Highlights).is_err());
}

#[test]
fn real_story_shots_preserve_synchronized_camera_hero_role_and_source_anchor() {
    let root = tempfile::tempdir().unwrap();
    let job = fixture(root.path());
    let plan = build_plan(&job, &clips()).unwrap();
    assert_eq!(plan["schema_version"], "d2h.timestamp-plan/1.2");
    assert_eq!(plan["time_basis"], "demo");
    let rows = plan["clips"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for field in ["start", "end", "take_group_id", "anchor"] {
        assert_eq!(rows[0][field], rows[1][field]);
    }
    assert_eq!(rows[0]["camera_mode"], "player_perspective");
    assert_eq!(rows[1]["camera_mode"], "hero_chase");
    assert_eq!(rows[0]["hero"], HERO);
    assert_eq!(rows[1]["hero"], OTHER);
    assert_eq!(rows[0]["take_role"], "primary");
    assert_eq!(rows[1]["take_role"], "alternate");
    assert_eq!(rows[0]["include_in_final"], true);
    assert_eq!(rows[1]["include_in_final"], false);
    assert_eq!(rows[0]["anchor"]["demo_tick"], 3000);
}

#[test]
fn a_changed_story_shot_source_group_or_hero_cannot_be_exported() {
    let root = tempfile::tempdir().unwrap();
    let job = fixture(root.path());
    for field in ["storyId", "shotId", "takeGroupId", "hero"] {
        let mut value = serde_json::to_value(clips()).unwrap();
        value[0][field] = json!("foreign");
        let bad: Vec<WtfClipInput> = serde_json::from_value(value).unwrap();
        assert!(build_plan(&job, &bad).is_err(), "{field}");
    }
    assert!(load_job(&root.path().join("wtf-jobs"), "../outside").is_err());
    let mut document = serde_json::to_value(&job.stories).unwrap();
    document["source_sha256"] = json!("wrong-source");
    d2_highlights_core::write_json_pretty(&job.directory.join("director/stories.json"), &document)
        .unwrap();
    assert!(load_job(&root.path().join("wtf-jobs"), JOB).is_err());
}

#[test]
fn invalid_ranges_and_primary_alternate_combinations_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let job = fixture(root.path());
    let mut bad = clips();
    bad[1].end_seconds = 109.0;
    assert!(validate_clips(&job, &bad, true).is_err());
    let mut bad = clips();
    bad[1].include_in_final = true;
    assert!(validate_clips(&job, &bad, true).is_err());
    let mut bad = clips();
    bad[0].take_role = RenderTakeRole::Alternate;
    assert!(validate_clips(&job, &bad, true).is_err());
    let mut swapped = clips();
    swapped[0].take_role = RenderTakeRole::Alternate;
    swapped[0].include_in_final = false;
    swapped[1].take_role = RenderTakeRole::Primary;
    swapped[1].include_in_final = true;
    assert!(validate_clips(&job, &swapped, true).is_err());
    let mut bad = clips();
    bad[0].start_seconds = f32::NAN;
    assert!(validate_clips(&job, &bad, true).is_err());
    let mut bad = clips();
    bad[0].camera_mode = ClipCameraMode::Directed;
    assert!(validate_clips(&job, &bad, true).is_err());
    assert!(build_plan(&job, &[]).is_err());
}

#[test]
fn saved_draft_round_trips_and_failed_save_does_not_replace_original() {
    let root = tempfile::tempdir().unwrap();
    let job = fixture(root.path());
    let draft = save_draft(&job, clips()).unwrap();
    assert_eq!(draft.clips.len(), 2);
    let path = job.directory.join("director/wtf-draft.json");
    let before = fs::read(&path).unwrap();
    let restored: WtfDraft = read_bound(&job.directory, &path, PLAN_LIMIT as u64).unwrap();
    assert_eq!(restored.source_sha256, job.manifest.source.sha256);
    assert_eq!(restored.clips[1].hero, OTHER);
    let mut bad = clips();
    bad[1].end_seconds = 108.0;
    assert!(save_draft(&job, bad).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut future: Value = serde_json::from_slice(&before).unwrap();
    future["schemaVersion"] = json!("future/99");
    d2_highlights_core::write_json_pretty(&path, &future).unwrap();
    let untouched = fs::read(&path).unwrap();
    assert!(save_draft(&job, clips()).is_err());
    assert_eq!(fs::read(path).unwrap(), untouched);
}

#[test]
fn json_export_never_overwrites_and_changed_dem_source_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let job = fixture(root.path());
    let plan = build_plan(&job, &clips()).unwrap();
    let path = root.path().join("plan.json");
    export_new(&path, &plan).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(export_new(&path, &json!({"wrong":true})).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(export_new(&root.path().join("file.exe"), &plan).is_err());
    fs::write(&job.manifest.source.path, b"PBDEMS2\0altered-content").unwrap();
    assert!(verify_source(&job).is_err());
}

#[test]
fn handoff_import_is_confined_to_unique_owned_session_and_preserves_user_jobs() {
    let root = tempfile::tempdir().unwrap();
    let job = fixture(root.path());
    let user_jobs = root.path().join("user-jobs");
    fs::create_dir(&user_jobs).unwrap();
    fs::write(user_jobs.join("keep.json"), b"original-user-draft").unwrap();
    let dem_before = fs::read(&job.manifest.source.path).unwrap();
    let importer = |replay: &Path, plan: &Path, jobs: &Path| -> Result<Value, String> {
        assert_eq!(fs::read(replay).unwrap(), dem_before);
        assert!(plan.starts_with(job.directory.join("handoffs")));
        assert!(jobs.starts_with(job.directory.join("handoffs")));
        assert_eq!(plan.parent(), jobs.parent());
        assert_eq!(jobs.file_name().unwrap(), "jobs");
        let value: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
        assert_eq!(value["schema_version"], "d2h.timestamp-plan/1.2");
        d2_highlights_core::write_json_pretty(
            &jobs.join(JOB).join("director/edit-plan.json"),
            &value,
        )
        .map_err(|error| error.to_string())?;
        Ok(json!({"jobId":JOB,"draftPath":jobs.join(JOB).join("director/edit-plan.json")}))
    };
    let (one, _) = prepare_handoff_with(&job, &clips(), importer).unwrap();
    let (two, _) = prepare_handoff_with(&job, &clips(), importer).unwrap();
    assert_ne!(one.session_directory, two.session_directory);
    assert!(!one.launched);
    assert_eq!(one.clip_count, 2);
    assert!(
        Path::new(&one.session_directory)
            .join("jobs")
            .join(JOB)
            .join("director/edit-plan.json")
            .is_file()
    );
    assert_eq!(
        fs::read(user_jobs.join("keep.json")).unwrap(),
        b"original-user-draft"
    );
    assert_eq!(fs::read(&job.manifest.source.path).unwrap(), dem_before);
}

#[test]
fn exported_plan_passes_actual_recorder_import_and_preserves_both_takes() {
    let root = tempfile::tempdir().unwrap();
    let job = fixture(root.path());
    let plan = build_plan(&job, &clips()).unwrap();
    let preview = timestamp_import::build_preview(
        &plan.to_string(),
        "wtf.json",
        &job.manifest.source,
        &job.timeline,
        None,
        None,
    )
    .unwrap();
    assert_eq!(preview.clips.len(), 2);
    let recorder_jobs = root.path().join("recorder-session/jobs");
    let recorder_job = recorder_jobs.join(JOB);
    for (path, value) in [
        (
            "manifest.json",
            serde_json::to_value(&job.manifest).unwrap(),
        ),
        (
            "timeline/combat-events.json",
            serde_json::to_value(&job.timeline).unwrap(),
        ),
        (
            "timeline/highlights.json",
            serde_json::to_value(&job.highlights).unwrap(),
        ),
    ] {
        d2_highlights_core::write_json_pretty(&recorder_job.join(path), &value).unwrap();
    }
    append_timestamp_preview(&recorder_jobs, JOB, preview).unwrap();
    let saved: Value = read_json(&recorder_job.join("director/edit-plan.json")).unwrap();
    let takes = saved["plans"][0]["clips"].as_array().unwrap();
    assert_eq!(takes.len(), 2);
    assert_eq!(takes[0]["candidate_id"], takes[1]["candidate_id"]);
    assert_eq!(takes[0]["take_group_id"], takes[1]["take_group_id"]);
    assert_eq!(takes[0]["camera_mode"], "player_perspective");
    assert_eq!(takes[1]["camera_mode"], "hero_chase");
    assert_eq!(takes[0]["view_hero"], HERO);
    assert_eq!(takes[1]["view_hero"], OTHER);
    assert_eq!(takes[0]["include_in_final"], true);
    assert_eq!(takes[1]["include_in_final"], false);
    assert!(!job.directory.join("director/edit-plan.json").exists());
}

#[test]
fn frame_grid_boundary_uses_an_anchor_inside_the_actual_imported_window() {
    let root = tempfile::tempdir().unwrap();
    let mut job = fixture(root.path());
    job.highlights.candidates[0].anchor_tick = 2701;
    job.highlights.candidates[0].peak_seconds = 90.02;
    job.timeline.events[0].tick = 2701;
    job.timeline.events[0].time_seconds = Some(90.02);
    let mut next = job.timeline.events[0].clone();
    next.tick = 2730;
    next.time_seconds = Some(91.0);
    job.timeline.events.push(next);
    let mut edited = clips();
    for clip in &mut edited {
        clip.start_seconds = 90.02;
        clip.end_seconds = 92.02;
    }
    let plan = build_plan(&job, &edited).unwrap();
    assert_eq!(plan["clips"][0]["start"], "00:01:30.033");
    assert_eq!(plan["clips"][0]["anchor"]["demo_tick"], 2730);
    assert_eq!(plan["clips"][0]["anchor"], plan["clips"][1]["anchor"]);
    let preview = timestamp_import::build_preview(
        &plan.to_string(),
        "wtf.json",
        &job.manifest.source,
        &job.timeline,
        None,
        None,
    )
    .unwrap();
    assert_eq!(preview.clips.len(), 2);
}

/// Explicit opt-in: all generated analysis/recorder data stays in a temporary
/// directory. This never starts either desktop application, Steam or Dota.
#[test]
#[ignore = "requires D2H_WTF_TEST_DEM with a real synchronized primary/alternate story; no GUI or game"]
fn real_dem_analysis_to_isolated_recorder_handoff_preserves_synchronized_takes() {
    use d2_highlights_core::StoryCameraMode;

    let replay = env::var_os("D2H_WTF_TEST_DEM")
        .map(PathBuf::from)
        .expect("set D2H_WTF_TEST_DEM to a matching real replay for this opt-in test");
    let source_before = d2_highlights_core::inspect_dem(&replay)
        .unwrap_or_else(|_| panic!("real replay inspection failed; verify the opt-in input"));
    let temporary = tempfile::tempdir().unwrap();
    let jobs_root = temporary.path().join("independent-wtf/jobs");
    let summary = d2_highlights_pipeline::analyze_dem_with_progress(&replay, &jobs_root, |_| {})
        .unwrap_or_else(|_| panic!("full real replay analysis failed"));
    let job = load_job(&jobs_root, &summary.job_id)
        .unwrap_or_else(|_| panic!("full analysis did not produce a source-consistent WTF job"));
    assert!(job.manifest.source.sha256 == source_before.sha256);

    let selected = job.stories.stories.iter().find_map(|story| {
        let mut groups = std::collections::BTreeMap::<&str, Vec<&d2_highlights_core::StoryShot>>::new();
        for shot in &story.shots {
            groups.entry(&shot.take_group_id).or_default().push(shot);
        }
        for shots in groups.values() {
            if !shots.iter().any(|shot| shot.take_role == StoryTakeRole::Primary)
                || !shots.iter().any(|shot| shot.take_role == StoryTakeRole::Alternate)
            {
                continue;
            }
            let clips: Vec<_> = shots.iter().map(|shot| {
                let camera = match shot.camera_mode {
                    StoryCameraMode::Directed => shot.fallback.camera_mode,
                    mode => mode,
                };
                WtfClipInput {
                    clip_id: format!("wtf-{}", shot.id), story_id: story.id.clone(), shot_id: shot.id.clone(),
                    hero: shot.target_hero.as_ref().or(shot.fallback.target_hero.as_ref()).unwrap_or(&story.primary_hero).clone(),
                    start_seconds: shot.source_start_seconds, end_seconds: shot.source_end_seconds,
                    camera_mode: if camera == StoryCameraMode::HeroChase { ClipCameraMode::HeroChase } else { ClipCameraMode::PlayerPerspective },
                    take_group_id: format!("wtf-{}-{}", story.id, shot.take_group_id),
                    take_role: if shot.take_role == StoryTakeRole::Primary { RenderTakeRole::Primary } else { RenderTakeRole::Alternate },
                    include_in_final: shot.include_in_default_cut,
                }
            }).collect();
            if validate_clips(&job, &clips, true).is_ok() && build_plan(&job, &clips).is_ok() {
                return Some(clips);
            }
        }
        None
    }).expect("the opt-in replay must contain a valid real story with synchronized primary and alternate takes; select a matching sample");
    assert!(selected.len() >= 2);
    save_draft(&job, selected.clone()).unwrap_or_else(|_| panic!("real WTF draft save failed"));
    let protected: Vec<_> = [
        "manifest.json",
        "timeline/combat-events.json",
        "timeline/highlights.json",
        "director/stories.json",
        "director/wtf-draft.json",
    ]
    .into_iter()
    .map(|relative| (relative, fs::read(job.directory.join(relative)).unwrap()))
    .collect();

    let plan = build_plan(&job, &selected)
        .unwrap_or_else(|_| panic!("real WTF recording plan failed recorder validation"));
    let expected = timestamp_import::build_preview(
        &plan.to_string(),
        "wtf-integration.json",
        &job.manifest.source,
        &job.timeline,
        None,
        None,
    )
    .unwrap_or_else(|_| panic!("real WTF recording preview failed"));
    let (receipt, returned_replay) = prepare_handoff(&job, &selected)
        .unwrap_or_else(|_| panic!("actual isolated recording import failed"));
    let session = Path::new(&receipt.session_directory);
    assert!(session.starts_with(job.directory.join("handoffs")));
    assert!(
        !receipt.launched,
        "prepare_handoff must never start a desktop app or game"
    );
    assert_eq!(receipt.clip_count, selected.len());
    let recorder_directory = session.join("jobs").join(&receipt.recording_job_id);
    let saved: Value = read_json(&recorder_directory.join("director/edit-plan.json"))
        .unwrap_or_else(|_| panic!("the real recorder draft was not saved"));
    assert!(saved["source_sha256"] == source_before.sha256);
    assert!(saved["active_mode"] == "default");
    let takes = saved["plans"][0]["clips"]
        .as_array()
        .expect("recorder draft clips missing");
    assert_eq!(takes.len(), expected.clips.len());
    assert_eq!(
        takes
            .iter()
            .filter(|clip| clip["take_role"] == "primary")
            .count(),
        1
    );
    assert!(
        takes
            .iter()
            .filter(|clip| clip["take_role"] == "alternate")
            .all(|clip| clip["include_in_final"] == false)
    );
    let primary = takes
        .iter()
        .find(|clip| clip["take_role"] == "primary")
        .unwrap();
    assert!(primary["include_in_final"] == true);
    for (take, preview) in takes.iter().zip(&expected.clips) {
        assert!(take["candidate_id"] == primary["candidate_id"]);
        assert!(take["take_group_id"] == primary["take_group_id"]);
        assert!(take["source_start_seconds"] == primary["source_start_seconds"]);
        assert!(take["source_end_seconds"] == primary["source_end_seconds"]);
        assert!(take["view_hero"] == preview.hero);
        assert!(take["camera_mode"] == serde_json::to_value(&preview.camera_mode).unwrap());
        assert!(take["take_role"] == serde_json::to_value(preview.take_role).unwrap());
        assert!(take["include_in_final"] == preview.include_in_final);
        assert!(take["imported"] == serde_json::to_value(&preview.imported).unwrap());
        assert!(
            take["source_start_seconds"]
                .as_f64()
                .is_some_and(|value| (value - f64::from(preview.start_seconds)).abs() < 0.0001)
        );
        assert!(
            take["source_end_seconds"]
                .as_f64()
                .is_some_and(|value| (value - f64::from(preview.end_seconds)).abs() < 0.0001)
        );
    }
    for (relative, original) in protected {
        assert!(
            fs::read(job.directory.join(relative)).unwrap() == original,
            "an isolated handoff must preserve the original WTF job artifacts"
        );
    }
    assert!(!job.directory.join("director/edit-plan.json").exists());
    let source_after = d2_highlights_core::inspect_dem(&returned_replay)
        .unwrap_or_else(|_| panic!("source replay inspection after handoff failed"));
    assert!(source_after.sha256 == source_before.sha256);
    println!(
        "real_dem_wtf_handoff: stories={}, shots={}, selected_takes={}, imported_takes={}, source_unchanged=true, launched=false",
        summary.stories.stories.len(),
        summary
            .stories
            .stories
            .iter()
            .map(|story| story.shots.len())
            .sum::<usize>(),
        selected.len(),
        takes.len()
    );
}
