use super::*;
use serde_json::{Value, json};

const ID: &str = "d2h-0123456789abcdef";
const HERO: &str = "npc_dota_hero_mirana";

fn fixture(root: &Path) -> PathBuf {
    let dir = root.join(ID);
    let source =
        json!({"path":"fixture.dem","byte_length":8,"sha256":"fixture-source","magic":"PBDEMS2"});
    for (relative, value) in [
        (
            "manifest.json",
            json!({"schema_version":"1.0","job_id":ID,"created_unix_seconds":0,"source":source,"stages":{}}),
        ),
        (
            "timeline/combat-events.json",
            json!({"schema_version":"1.4","source_sha256":"fixture-source","parser":{"name":"fixture","version":"1"},
            "replay":{"playback_ticks":18000,"playback_time_seconds":600,"match_id":42,"game_build":1,
                "players":[{"slot":1,"hero_name":HERO,"game_team":2,"is_fake_client":false}]},
            "events":[{"tick":3000,"time_seconds":100,"event_type":"DotaCombatlogDamage","attacker":HERO,"target":"npc_dota_neutral_centaur_khan","value":100,"assist_players":[]}]}),
        ),
        (
            "timeline/highlights.json",
            json!({"schema_version":"1.4","source_sha256":"fixture-source","detector":{"name":"fixture","version":"1"},
            "candidates":[{"id":"hk-001","rank":1,"kind":"hero_kill_sequence","title":"fixture","score":1,"start_seconds":90,"peak_seconds":100,"end_seconds":110,
                "hero_deaths":1,"anchor_tick":3000,"primary_hero":HERO,"participants":[HERO],"reasons":[]}]}),
        ),
    ] {
        d2_highlights_core::write_json_pretty(&dir.join(relative), &value).unwrap();
    }
    dir
}

fn request() -> SaveEditPlanRequest {
    serde_json::from_value(json!({"jobId":ID,"activeMode":"default","plans":[{"mode":"default","selectedHero":HERO,
        "clips":[{"clipId":"clip-highlight-1","candidateId":"hk-001","viewHero":HERO,"cameraMode":"player_perspective",
            "shortSourceStartSeconds":90,"shortSourceEndSeconds":110,"sourceStartSeconds":90,"sourceEndSeconds":110}]}]})).unwrap()
}

fn old_document(wtf: Value) -> Value {
    json!({"schema_version":EDIT_PLAN_SCHEMA_VERSION,"job_id":ID,"source_sha256":"fixture-source","active_mode":"wtf",
        "plans":[{"mode":"default","clips":[]},wtf],"mode":"dual","updated_unix_seconds":0,"clips":[],"settings":RenderSettings::default()})
}

fn opaque_wtf() -> Value {
    json!({"mode":"wtf","clips":[{"retired_candidate":"kept-without-decoding"}],
        "custom_future_setting":{"style":"original","steps":[1,2,3]}})
}

#[test]
fn new_highlight_save_does_not_create_a_wtf_workspace() {
    let root = tempfile::tempdir().unwrap();
    let dir = fixture(root.path());
    let result = save_edit_plan_to(root.path(), request()).unwrap();
    let saved: Value = read_json(&dir.join("director/edit-plan.json")).unwrap();
    assert_eq!(result.selected_clip_count, 1);
    assert_eq!(result.total_duration_seconds, 20.0);
    assert_eq!(saved["plans"].as_array().unwrap().len(), 1);
    assert_eq!(saved["active_mode"], "default");
}

#[test]
fn highlight_save_and_load_keep_old_wtf_json_opaque() {
    let root = tempfile::tempdir().unwrap();
    let dir = fixture(root.path());
    let path = dir.join("director/edit-plan.json");
    let wtf = opaque_wtf();
    d2_highlights_core::write_json_pretty(&path, &old_document(wtf.clone())).unwrap();
    save_edit_plan_to(root.path(), request()).unwrap();
    let saved: Value = read_json(&path).unwrap();
    assert_eq!(saved["plans"][1], wtf);
    let loaded = load_highlight_plan_at(&path, &dir).unwrap().unwrap();
    assert_eq!(loaded.active_mode, EditPlanMode::Default);
    assert_eq!(loaded.plans.len(), 1);
    assert_eq!(loaded.plans[0].clips[0].clip_id, "clip-highlight-1");
    let state = AppState {
        jobs_root: root.path().into(),
        dota2_runtime: Arc::new(Mutex::new(Dota2Runtime {
            executable: Some("not-executed-dota2.exe".into()),
            source: Dota2PathSource::Missing,
        })),
        dota2_settings_path: PathBuf::new(),
        video_output: video_output::Settings::default(),
        ffmpeg_exe: Some("not-executed-ffmpeg.exe".into()),
        ffprobe_exe: Some("not-executed-ffprobe.exe".into()),
        render_runtime: Arc::new(Mutex::new(RenderRuntime::default())),
        activities: Activities::default(),
    };
    let render_request = build_render_request(ID, &state).unwrap();
    assert_eq!(render_request.clips.len(), 1);
    assert_eq!(render_request.clips[0].clip_id, "clip-highlight-1");
}

#[test]
fn reanalysis_keeps_exact_backup_and_preserves_wtf_for_the_next_save() {
    let root = tempfile::tempdir().unwrap();
    let dir = fixture(root.path());
    let path = dir.join("director/edit-plan.json");
    let wtf = opaque_wtf();
    d2_highlights_core::write_json_pretty(&path, &old_document(wtf.clone())).unwrap();
    let before = fs::read(&path).unwrap();
    let backup = archive_edit_plan_for_reanalysis(root.path(), ID)
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(backup).unwrap(), before);
    assert!(load_highlight_plan_at(&path, &dir).unwrap().is_none());
    save_edit_plan_to(root.path(), request()).unwrap();
    let saved: Value = read_json(&path).unwrap();
    assert_eq!(saved["plans"][1], wtf);
    assert_eq!(saved["plans"][0]["mode"], "default");
}

#[test]
fn unsupported_saved_schema_is_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let dir = fixture(root.path());
    let path = dir.join("director/edit-plan.json");
    let mut previous = old_document(opaque_wtf());
    previous["schema_version"] = json!("future/99");
    d2_highlights_core::write_json_pretty(&path, &previous).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(save_edit_plan_to(root.path(), request()).is_err());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn retired_wtf_edit_requests_are_rejected_without_touching_the_file() {
    let root = tempfile::tempdir().unwrap();
    let dir = fixture(root.path());
    let path = dir.join("director/edit-plan.json");
    d2_highlights_core::write_json_pretty(&path, &old_document(opaque_wtf())).unwrap();
    let before = fs::read(&path).unwrap();
    let mut incoming = request();
    incoming.active_mode = EditPlanMode::Wtf;
    assert!(save_edit_plan_to(root.path(), incoming).is_err());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn timestamp_handoff_targets_highlights_and_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let dir = fixture(root.path());
    let timeline: TimelineDocument = read_json(&dir.join("timeline/combat-events.json")).unwrap();
    let manifest: JobManifest = read_json(&dir.join("manifest.json")).unwrap();
    let input = json!({"schema_version":"d2h.timestamp-plan/1.1","match_id":"42","hero":HERO,"time_basis":"demo",
        "source":{"sha256":"fixture-source"},"clips":[{"id":"external-1","title":"external clip","start":"01:35","end":"01:45"}]}).to_string();
    let make_preview = || {
        timestamp_import::build_preview(
            &input,
            "handoff.json",
            &manifest.source,
            &timeline,
            None,
            None,
        )
        .unwrap()
    };
    let first = append_timestamp_preview(root.path(), ID, make_preview()).unwrap();
    let second = append_timestamp_preview(root.path(), ID, make_preview()).unwrap();
    assert_eq!(first["module"], "highlights");
    assert_eq!(first["totalClipCount"], 1);
    assert_eq!(second["addedClipIds"], json!([]));
    let loaded = load_highlight_plan_at(&dir.join("director/edit-plan.json"), &dir)
        .unwrap()
        .unwrap();
    let clip = &loaded.plans[0].clips[0];
    assert_eq!(clip.imported.as_ref().unwrap().anchor_tick, 3000);
    assert_eq!(clip.source_start_seconds, 95.0);
    assert_eq!(clip.source_end_seconds, 105.0);
    assert!(!dir.join("director/story-edit-plan.json").exists());
}
