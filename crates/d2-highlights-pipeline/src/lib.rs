use d2_highlights_core::{
    DIRECTOR_SCHEMA_VERSION, DemSource, DirectorDocument, HIGHLIGHT_SCHEMA_VERSION,
    HighlightCandidate, HighlightDocument, IngestError, JobManifest, ReplayMetadata,
    STORY_SCHEMA_VERSION, StageStatus, StoryDocument, TIMELINE_SCHEMA_VERSION, TimelineDocument,
    ingest_dem, update_stage, write_json_pretty,
};
use d2_highlights_detector::{DETECTOR_NAME, DETECTOR_VERSION, detect_highlights};
use d2_highlights_director::{StoryValidationError, build_director_plan, build_story_document};
use d2_highlights_parser_source2::{
    PARSER_ADAPTER_CACHE_VERSION, ParserAdapterError, SOURCE2_DEMO_VERSION, parse_combat_timeline,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const ANALYSIS_CACHE_VERSION: &str = "2";

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct AnalysisCacheStamp {
    version: String,
    source_sha256: String,
    detector_version: String,
    parser_version: String,
    parser_adapter_version: String,
    timeline_sha256: String,
    highlights_sha256: String,
    stories_sha256: String,
    director_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisProgress {
    pub stage: String,
    pub status: StageStatus,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisSummary {
    pub job_id: String,
    pub job_dir: String,
    pub source: DemSource,
    pub replay: ReplayMetadata,
    pub event_count: usize,
    pub highlights: HighlightDocument,
    pub stories: StoryDocument,
    pub director: DirectorDocument,
    pub reused_existing_job: bool,
}

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("DEM ingest failed: {0}")]
    Ingest(#[from] IngestError),
    #[error("DEM parsing failed: {0}")]
    Parse(#[from] ParserAdapterError),
    #[error("story planning failed: {0}")]
    Story(#[from] StoryValidationError),
    #[error("unable to read pipeline artifact {path}: {source}")]
    ReadArtifact {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid pipeline artifact {path}: {source}")]
    DecodeArtifact {
        path: PathBuf,
        source: serde_json::Error,
    },
}

pub fn analyze_dem(dem_path: &Path, jobs_root: &Path) -> Result<AnalysisSummary, PipelineError> {
    analyze_dem_with_progress(dem_path, jobs_root, |_| {})
}

pub fn analyze_dem_with_progress<F>(
    dem_path: &Path,
    jobs_root: &Path,
    mut report: F,
) -> Result<AnalysisSummary, PipelineError>
where
    F: FnMut(AnalysisProgress),
{
    report(progress("ingest", StageStatus::Running, "正在检查录像文件"));
    let ingested = ingest_dem(dem_path, jobs_root)?;
    report(progress(
        "ingest",
        StageStatus::Complete,
        if ingested.reused_existing_job {
            "已找到同一录像的任务"
        } else {
            "录像校验完成"
        },
    ));

    let timeline_path = ingested.job_dir.join("timeline").join("combat-events.json");
    let highlights_path = ingested.job_dir.join("timeline").join("highlights.json");
    let stories_path = ingested.job_dir.join("director").join("stories.json");
    let director_path = ingested.job_dir.join("director").join("plan.json");
    let stamp_path = ingested.job_dir.join("analysis-cache.json");
    let stamp = read_json::<AnalysisCacheStamp>(&stamp_path).ok();

    if completed_artifacts_exist(
        &ingested.manifest,
        &timeline_path,
        &highlights_path,
        &stories_path,
        &director_path,
    ) && stamp.as_ref().is_some_and(|stamp| {
        cache_stamp_matches(
            stamp,
            &ingested.manifest.source.sha256,
            &timeline_path,
            &highlights_path,
            &stories_path,
            &director_path,
        )
    }) {
        report(progress(
            "complete",
            StageStatus::Complete,
            "已复用完成的分析结果",
        ));
        return load_summary(
            &ingested.manifest,
            &ingested.job_dir,
            &timeline_path,
            &highlights_path,
            &stories_path,
            &director_path,
            true,
        );
    }

    let cached_timeline = read_json::<TimelineDocument>(&timeline_path)
        .ok()
        .filter(|timeline| {
            stage_complete(&ingested.manifest, "parse")
                && timeline.schema_version == TIMELINE_SCHEMA_VERSION
                && timeline.source_sha256 == ingested.manifest.source.sha256
                && timeline.parser.name == "source2-demo"
                && timeline.parser.version == SOURCE2_DEMO_VERSION
                && stamp.as_ref().is_none_or(|stamp| {
                    stamp.parser_adapter_version == PARSER_ADAPTER_CACHE_VERSION
                })
        });
    let timeline_rebuilt = cached_timeline.is_none();
    let timeline = if let Some(timeline) = cached_timeline {
        report(progress("parse", StageStatus::Complete, "已复用比赛事件"));
        timeline
    } else {
        update_stage(&ingested.job_dir, "parse", StageStatus::Running, None, None)?;
        report(progress("parse", StageStatus::Running, "正在读取比赛事件"));
        let timeline = match parse_combat_timeline(dem_path, &ingested.manifest.source.sha256) {
            Ok(timeline) => timeline,
            Err(error) => {
                let _ = update_stage(
                    &ingested.job_dir,
                    "parse",
                    StageStatus::Failed,
                    None,
                    Some(error.to_string()),
                );
                report(progress("parse", StageStatus::Failed, "录像解析失败"));
                return Err(error.into());
            }
        };
        write_json_pretty(&timeline_path, &timeline)?;
        update_stage(
            &ingested.job_dir,
            "parse",
            StageStatus::Complete,
            Some("timeline/combat-events.json".to_string()),
            None,
        )?;
        report(progress("parse", StageStatus::Complete, "比赛事件读取完成"));
        timeline
    };

    let highlights = if stage_complete(&ingested.manifest, "detect")
        && highlight_artifact_matches(&highlights_path)
        && !timeline_rebuilt
        && stamp.as_ref().is_some_and(|stamp| {
            detection_dependencies_match(
                stamp,
                &ingested.manifest.source.sha256,
                &timeline_path,
                &highlights_path,
            )
        }) {
        report(progress("detect", StageStatus::Complete, "已复用高光候选"));
        read_json(&highlights_path)?
    } else {
        update_stage(
            &ingested.job_dir,
            "detect",
            StageStatus::Running,
            None,
            None,
        )?;
        report(progress("detect", StageStatus::Running, "正在寻找高光片段"));
        let previous = read_json::<HighlightDocument>(&highlights_path)
            .ok()
            .filter(|document| document.source_sha256 == timeline.source_sha256);
        let mut highlights = detect_highlights(&timeline, 20, 18.0);
        if let Some(previous) = previous {
            preserve_candidate_ids(&mut highlights, &previous, &timeline);
        } else if ingested.job_dir.join("director/edit-plan.json").is_file() {
            // An unreadable/missing or foreign candidate document cannot establish
            // what an old sequential ID meant. Keep the draft untouched and use
            // identity-derived IDs so it cannot silently point at a new event.
            let untrusted_previous = HighlightDocument {
                schema_version: highlights.schema_version.clone(),
                source_sha256: highlights.source_sha256.clone(),
                detector: highlights.detector.clone(),
                candidates: Vec::new(),
            };
            preserve_candidate_ids(&mut highlights, &untrusted_previous, &timeline);
        }
        write_json_pretty(&highlights_path, &highlights)?;
        update_stage(
            &ingested.job_dir,
            "detect",
            StageStatus::Complete,
            Some("timeline/highlights.json".to_string()),
            None,
        )?;
        report(progress(
            "detect",
            StageStatus::Complete,
            "高光候选已经整理好",
        ));
        highlights
    };

    update_stage(
        &ingested.job_dir,
        "direct",
        StageStatus::Running,
        None,
        None,
    )?;
    report(progress("direct", StageStatus::Running, "正在编排剧情节奏"));
    let stories = match build_story_document(&timeline, &highlights) {
        Ok(stories) => stories,
        Err(error) => {
            let _ = update_stage(
                &ingested.job_dir,
                "direct",
                StageStatus::Failed,
                None,
                Some(error.to_string()),
            );
            report(progress("direct", StageStatus::Failed, "故事证据校验失败"));
            return Err(error.into());
        }
    };
    let director = build_director_plan(&highlights, "comic_hype_v1", 10, 90.0);
    write_json_pretty(&stories_path, &stories)?;
    write_json_pretty(&director_path, &director)?;
    update_stage(
        &ingested.job_dir,
        "direct",
        StageStatus::Complete,
        Some("director/plan.json".to_string()),
        None,
    )?;
    write_json_pretty(
        &stamp_path,
        &AnalysisCacheStamp {
            version: ANALYSIS_CACHE_VERSION.into(),
            source_sha256: ingested.manifest.source.sha256.clone(),
            detector_version: DETECTOR_VERSION.into(),
            parser_version: SOURCE2_DEMO_VERSION.into(),
            parser_adapter_version: PARSER_ADAPTER_CACHE_VERSION.into(),
            timeline_sha256: artifact_hash(&timeline_path)?,
            highlights_sha256: artifact_hash(&highlights_path)?,
            stories_sha256: artifact_hash(&stories_path)?,
            director_sha256: artifact_hash(&director_path)?,
        },
    )?;
    report(progress("direct", StageStatus::Complete, "剧情编排完成"));
    report(progress("complete", StageStatus::Complete, "录像分析完成"));

    Ok(AnalysisSummary {
        job_id: ingested.manifest.job_id,
        job_dir: ingested.job_dir.display().to_string(),
        source: ingested.manifest.source,
        replay: timeline.replay,
        event_count: timeline.events.len(),
        highlights,
        stories,
        director,
        reused_existing_job: false,
    })
}

fn artifact_hash(path: &Path) -> Result<String, PipelineError> {
    let bytes = fs::read(path).map_err(|source| PipelineError::ReadArtifact {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn detection_dependencies_match(
    stamp: &AnalysisCacheStamp,
    source_sha256: &str,
    timeline: &Path,
    highlights: &Path,
) -> bool {
    stamp.version == ANALYSIS_CACHE_VERSION
        && stamp.source_sha256 == source_sha256
        && stamp.detector_version == DETECTOR_VERSION
        && stamp.parser_version == SOURCE2_DEMO_VERSION
        && stamp.parser_adapter_version == PARSER_ADAPTER_CACHE_VERSION
        && artifact_hash(timeline).ok().as_deref() == Some(stamp.timeline_sha256.as_str())
        && artifact_hash(highlights).ok().as_deref() == Some(stamp.highlights_sha256.as_str())
}

fn cache_stamp_matches(
    stamp: &AnalysisCacheStamp,
    source_sha256: &str,
    timeline: &Path,
    highlights: &Path,
    stories: &Path,
    director: &Path,
) -> bool {
    detection_dependencies_match(stamp, source_sha256, timeline, highlights)
        && artifact_hash(stories).ok().as_deref() == Some(stamp.stories_sha256.as_str())
        && artifact_hash(director).ok().as_deref() == Some(stamp.director_sha256.as_str())
}

fn candidate_identity(
    candidate: &HighlightCandidate,
    timeline: &TimelineDocument,
) -> (String, Option<String>, Vec<u32>) {
    if let Some(sequence) = &candidate.kill_sequence {
        return (
            "hero_kill_sequence".into(),
            Some(sequence.hero.clone()),
            sequence.kills.iter().map(|kill| kill.death_tick).collect(),
        );
    }
    if matches!(
        candidate.kind.as_str(),
        "first_blood" | "multikill" | "team_fight" | "trade" | "pickoff"
    ) {
        // Old generic peaks can be a reincarnation. Reuse an ID only if the same
        // last final death still anchors the corrected cluster.
        let anchor = timeline
            .events
            .iter()
            .filter(|event| {
                event.is_final_hero_death()
                    && event.time_seconds.is_some_and(|time| {
                        time >= candidate.start_seconds && time <= candidate.peak_seconds
                    })
            })
            .max_by_key(|event| event.tick)
            .map_or(candidate.anchor_tick, |event| event.tick);
        return ("combat_deaths".into(), None, vec![anchor]);
    }
    (
        candidate.kind.clone(),
        candidate.primary_hero.clone(),
        vec![candidate.anchor_tick],
    )
}

fn preserve_candidate_ids(
    current: &mut HighlightDocument,
    previous: &HighlightDocument,
    timeline: &TimelineDocument,
) {
    let mut reserved = previous
        .candidates
        .iter()
        .map(|candidate| candidate.id.clone())
        .collect::<BTreeSet<_>>();
    let mut assigned = BTreeSet::new();
    for candidate in &mut current.candidates {
        let identity = candidate_identity(candidate, timeline);
        let matches = previous
            .candidates
            .iter()
            .filter(|old| candidate_identity(old, timeline) == identity)
            .collect::<Vec<_>>();
        if matches.len() == 1 && assigned.insert(matches[0].id.clone()) {
            candidate.id = matches[0].id.clone();
            continue;
        }
        // Never recycle an obsolete ID: an old saved clip must fail visibly rather
        // than silently target a different event after ranking or detector changes.
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&identity).expect("serializable candidate identity"))
        );
        let mut collision = 0_u64;
        loop {
            let id = if collision == 0 {
                format!("hl-revised-{}", &digest[..20])
            } else {
                format!("hl-revised-{}-{collision}", &digest[..20])
            };
            collision += 1;
            if reserved.insert(id.clone()) {
                assigned.insert(id.clone());
                candidate.id = id;
                break;
            }
        }
    }
}

fn progress(stage: &str, status: StageStatus, message: &str) -> AnalysisProgress {
    AnalysisProgress {
        stage: stage.to_string(),
        status,
        message: message.to_string(),
    }
}

fn completed_artifacts_exist(
    manifest: &JobManifest,
    timeline_path: &Path,
    highlights_path: &Path,
    stories_path: &Path,
    director_path: &Path,
) -> bool {
    ["parse", "detect", "direct"].iter().all(|stage| {
        manifest
            .stages
            .get(*stage)
            .is_some_and(|record| record.status == StageStatus::Complete)
    }) && timeline_path.is_file()
        && highlights_path.is_file()
        && stories_path.is_file()
        && director_path.is_file()
        && artifact_schema_matches(timeline_path, TIMELINE_SCHEMA_VERSION)
        && highlight_artifact_matches(highlights_path)
        && story_artifact_matches(stories_path)
        && artifact_schema_matches(director_path, DIRECTOR_SCHEMA_VERSION)
}

fn stage_complete(manifest: &JobManifest, stage: &str) -> bool {
    manifest
        .stages
        .get(stage)
        .is_some_and(|record| record.status == StageStatus::Complete)
}

fn artifact_schema_matches(path: &Path, expected: &str) -> bool {
    #[derive(Deserialize)]
    struct ArtifactSchema {
        schema_version: String,
    }

    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ArtifactSchema>(&bytes).ok())
        .is_some_and(|artifact| artifact.schema_version == expected)
}

fn highlight_artifact_matches(path: &Path) -> bool {
    #[derive(Deserialize)]
    struct DetectorArtifact {
        name: String,
        version: String,
    }

    #[derive(Deserialize)]
    struct HighlightArtifact {
        schema_version: String,
        detector: DetectorArtifact,
    }

    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<HighlightArtifact>(&bytes).ok())
        .is_some_and(|artifact| {
            artifact.schema_version == HIGHLIGHT_SCHEMA_VERSION
                && artifact.detector.name == DETECTOR_NAME
                && artifact.detector.version == DETECTOR_VERSION
        })
}

fn story_artifact_matches(path: &Path) -> bool {
    #[derive(Deserialize)]
    struct DetectorArtifact {
        name: String,
        version: String,
    }

    #[derive(Deserialize)]
    struct StoryArtifact {
        schema_version: String,
        detector: DetectorArtifact,
    }

    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StoryArtifact>(&bytes).ok())
        .is_some_and(|artifact| {
            artifact.schema_version == STORY_SCHEMA_VERSION
                && artifact.detector.name == DETECTOR_NAME
                && artifact.detector.version == DETECTOR_VERSION
        })
}

fn load_summary(
    manifest: &JobManifest,
    job_dir: &Path,
    timeline_path: &Path,
    highlights_path: &Path,
    stories_path: &Path,
    director_path: &Path,
    reused_existing_job: bool,
) -> Result<AnalysisSummary, PipelineError> {
    let timeline: TimelineDocument = read_json(timeline_path)?;
    let highlights = read_json(highlights_path)?;
    let stories = read_json(stories_path)?;
    let director = read_json(director_path)?;

    Ok(AnalysisSummary {
        job_id: manifest.job_id.clone(),
        job_dir: job_dir.display().to_string(),
        source: manifest.source.clone(),
        replay: timeline.replay,
        event_count: timeline.events.len(),
        highlights,
        stories,
        director,
        reused_existing_job,
    })
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, PipelineError> {
    let bytes = fs::read(path).map_err(|source| PipelineError::ReadArtifact {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| PipelineError::DecodeArtifact {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use d2_highlights_core::{MANIFEST_SCHEMA_VERSION, StageRecord};
    use std::collections::BTreeMap;

    fn test_timeline(source: &str) -> TimelineDocument {
        serde_json::from_value(serde_json::json!({
            "schema_version": TIMELINE_SCHEMA_VERSION, "source_sha256": source,
            "parser": {"name": "source2-demo", "version": "0.5.8"},
            "replay": {"playback_ticks": 30000, "playback_time_seconds": 1000.0, "game_build": 1,
                "players": [{"slot": 0, "hero_name": "npc_dota_hero_mirana", "game_team": 2, "is_fake_client": false},
                            {"slot": 1, "hero_name": "npc_dota_hero_axe", "game_team": 3, "is_fake_client": false}]},
            "events": [{"tick": 3000, "time_seconds": 100.0, "event_type": "DotaCombatlogDeath", "attacker": "npc_dota_hero_mirana", "target": "npc_dota_hero_axe", "target_is_hero": true, "will_reincarnate": false, "assist_players": []}],
            "tree_orders": [], "temporary_trees": [], "salutes": [], "state_snapshots": []
        })).unwrap()
    }

    fn cache_stamp_for(source: &str, directory: &Path) -> AnalysisCacheStamp {
        AnalysisCacheStamp {
            version: ANALYSIS_CACHE_VERSION.into(),
            source_sha256: source.into(),
            detector_version: DETECTOR_VERSION.into(),
            parser_version: SOURCE2_DEMO_VERSION.into(),
            parser_adapter_version: PARSER_ADAPTER_CACHE_VERSION.into(),
            timeline_sha256: artifact_hash(&directory.join("timeline/combat-events.json")).unwrap(),
            highlights_sha256: artifact_hash(&directory.join("timeline/highlights.json")).unwrap(),
            stories_sha256: artifact_hash(&directory.join("director/stories.json")).unwrap(),
            director_sha256: artifact_hash(&directory.join("director/plan.json")).unwrap(),
        }
    }

    #[test]
    fn changed_upstream_timeline_rebuilds_detection_without_retargeting_saved_clip() {
        let temp = tempfile::tempdir().unwrap();
        let dem = temp.path().join("source.dem");
        fs::write(&dem, b"PBDEMS2\0\x10\0\0\0\0\0\0\0").unwrap();
        let jobs = temp.path().join("jobs");
        let ingested = ingest_dem(&dem, &jobs).unwrap();
        let directory = &ingested.job_dir;
        let mut timeline = test_timeline(&ingested.manifest.source.sha256);
        let highlights = detect_highlights(&timeline, 20, 18.0);
        let original_kill = highlights
            .candidates
            .iter()
            .find(|candidate| candidate.kind == "hero_kill_sequence")
            .unwrap();
        let original_id = original_kill.id.clone();
        write_json_pretty(&directory.join("timeline/combat-events.json"), &timeline).unwrap();
        write_json_pretty(&directory.join("timeline/highlights.json"), &highlights).unwrap();
        write_json_pretty(
            &directory.join("director/stories.json"),
            &build_story_document(&timeline, &highlights).unwrap(),
        )
        .unwrap();
        write_json_pretty(
            &directory.join("director/plan.json"),
            &build_director_plan(&highlights, "comic_hype_v1", 10, 90.0),
        )
        .unwrap();
        for stage in ["parse", "detect", "direct"] {
            update_stage(directory, stage, StageStatus::Complete, None, None).unwrap();
        }
        let stamp = cache_stamp_for(&timeline.source_sha256, directory);
        write_json_pretty(&directory.join("analysis-cache.json"), &stamp).unwrap();
        let plan_path = directory.join("director/edit-plan.json");
        let saved_clip = serde_json::to_vec(&serde_json::json!({"candidate_id": original_id, "source_start_seconds": 95.0, "source_end_seconds": 103.0})).unwrap();
        fs::write(&plan_path, &saved_clip).unwrap();
        assert!(analyze_dem(&dem, &jobs).unwrap().reused_existing_job);

        let mut earlier_kill = timeline.events[0].clone();
        earlier_kill.time_seconds = Some(50.0);
        earlier_kill.tick = 1500;
        timeline.events.insert(0, earlier_kill);
        write_json_pretty(&directory.join("timeline/combat-events.json"), &timeline).unwrap();
        let mut stages = Vec::new();
        let refreshed = analyze_dem_with_progress(&dem, &jobs, |progress| {
            stages.push((progress.stage, progress.status))
        })
        .unwrap();
        assert!(stages.contains(&("detect".to_string(), StageStatus::Running)));
        assert!(!refreshed.reused_existing_job);
        let preserved = refreshed
            .highlights
            .candidates
            .iter()
            .find(|candidate| candidate.id == original_id)
            .unwrap();
        assert_eq!(preserved.anchor_tick, 3000);
        assert!(
            refreshed
                .highlights
                .candidates
                .iter()
                .any(|candidate| candidate.anchor_tick == 1500)
        );
        assert_eq!(fs::read(&plan_path).unwrap(), saved_clip);
        assert!(analyze_dem(&dem, &jobs).unwrap().reused_existing_job);

        let mut stale = cache_stamp_for(&timeline.source_sha256, directory);
        stale.detector_version = "old".into();
        assert!(!detection_dependencies_match(
            &stale,
            &timeline.source_sha256,
            &directory.join("timeline/combat-events.json"),
            &directory.join("timeline/highlights.json")
        ));
        stale.detector_version = DETECTOR_VERSION.into();
        stale.parser_adapter_version = "old adapter".into();
        assert!(!detection_dependencies_match(
            &stale,
            &timeline.source_sha256,
            &directory.join("timeline/combat-events.json"),
            &directory.join("timeline/highlights.json")
        ));
        stale.parser_adapter_version = PARSER_ADAPTER_CACHE_VERSION.into();
        stale.parser_version = "old parser".into();
        assert!(!detection_dependencies_match(
            &stale,
            &timeline.source_sha256,
            &directory.join("timeline/combat-events.json"),
            &directory.join("timeline/highlights.json")
        ));
        assert!(!detection_dependencies_match(
            &stamp,
            "different source",
            &directory.join("timeline/combat-events.json"),
            &directory.join("timeline/highlights.json")
        ));
    }

    #[test]
    fn removed_event_ids_are_not_reused_for_different_clips() {
        let mut timeline = test_timeline("same source");
        let previous = detect_highlights(&timeline, 20, 18.0);
        timeline.events[0].tick = 6000;
        timeline.events[0].time_seconds = Some(200.0);
        let mut current = detect_highlights(&timeline, 20, 18.0);
        preserve_candidate_ids(&mut current, &previous, &timeline);
        let old_ids = previous
            .candidates
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect::<BTreeSet<_>>();
        assert!(
            current
                .candidates
                .iter()
                .all(|candidate| !old_ids.contains(candidate.id.as_str()))
        );
    }

    #[test]
    fn recovering_without_trustworthy_highlights_never_reuses_saved_sequential_ids() {
        for previous_kind in ["missing", "unreadable", "different_source"] {
            let temp = tempfile::tempdir().unwrap();
            let dem = temp.path().join("source.dem");
            fs::write(&dem, b"PBDEMS2\0\x10\0\0\0\0\0\0\0").unwrap();
            let jobs = temp.path().join("jobs");
            let ingested = ingest_dem(&dem, &jobs).unwrap();
            let timeline = test_timeline(&ingested.manifest.source.sha256);
            let previous = detect_highlights(&timeline, 20, 18.0);
            let original_ids = previous
                .candidates
                .iter()
                .map(|candidate| candidate.id.clone())
                .collect::<BTreeSet<_>>();
            let directory = &ingested.job_dir;
            write_json_pretty(&directory.join("timeline/combat-events.json"), &timeline).unwrap();
            update_stage(directory, "parse", StageStatus::Complete, None, None).unwrap();
            let highlights_path = directory.join("timeline/highlights.json");
            match previous_kind {
                "unreadable" => fs::write(&highlights_path, b"{").unwrap(),
                "different_source" => {
                    let mut foreign = previous;
                    foreign.source_sha256 = "another replay".into();
                    write_json_pretty(&highlights_path, &foreign).unwrap();
                }
                _ => {}
            }
            let plan = directory.join("director/edit-plan.json");
            let saved_plan = serde_json::to_vec(&serde_json::json!({
                "clips": [{"candidate_id": "hk-001", "source_start_seconds": 95.0, "source_end_seconds": 103.0}]
            }))
            .unwrap();
            fs::write(&plan, &saved_plan).unwrap();
            let recovered = analyze_dem(&dem, &jobs).unwrap();
            assert!(recovered.highlights.candidates.iter().all(|candidate| {
                candidate.id.starts_with("hl-revised-") && !original_ids.contains(&candidate.id)
            }));
            assert_eq!(fs::read(plan).unwrap(), saved_plan);
            let reopened = analyze_dem(&dem, &jobs).unwrap();
            assert!(reopened.reused_existing_job);
            assert_eq!(recovered.highlights, reopened.highlights);
        }
    }

    #[test]
    fn removing_reincarnation_peak_preserves_the_original_final_death_clip_reference() {
        let mut timeline = test_timeline("same source");
        let mut reincarnation = timeline.events[0].clone();
        reincarnation.tick = 3150;
        reincarnation.time_seconds = Some(105.0);
        reincarnation.will_reincarnate = Some(true);
        timeline.events.push(reincarnation);
        let mut previous = detect_highlights(&timeline, 20, 18.0);
        let old = previous
            .candidates
            .iter_mut()
            .find(|candidate| candidate.kind == "pickoff")
            .unwrap();
        let saved_id = old.id.clone();
        old.anchor_tick = 3150;
        old.peak_seconds = 105.0;
        old.end_seconds = 113.0;
        old.hero_deaths = 2;
        let mut current = detect_highlights(&timeline, 20, 18.0);
        preserve_candidate_ids(&mut current, &previous, &timeline);
        let preserved = current
            .candidates
            .iter()
            .find(|candidate| candidate.id == saved_id)
            .unwrap();
        assert_eq!(preserved.anchor_tick, 3000);
        assert_eq!(preserved.hero_deaths, 1);
    }

    #[test]
    fn completed_artifacts_require_every_analysis_stage() {
        let temp = tempfile::tempdir().unwrap();
        let timeline = temp.path().join("timeline.json");
        let highlights = temp.path().join("highlights.json");
        let stories = temp.path().join("stories.json");
        let director = temp.path().join("director.json");
        fs::write(
            &timeline,
            format!(r#"{{"schema_version":"{TIMELINE_SCHEMA_VERSION}"}}"#),
        )
        .unwrap();
        fs::write(
            &highlights,
            format!(
                r#"{{"schema_version":"{HIGHLIGHT_SCHEMA_VERSION}","detector":{{"name":"{DETECTOR_NAME}","version":"{DETECTOR_VERSION}"}}}}"#
            ),
        )
        .unwrap();
        fs::write(
            &stories,
            format!(
                r#"{{"schema_version":"{STORY_SCHEMA_VERSION}","detector":{{"name":"{DETECTOR_NAME}","version":"{DETECTOR_VERSION}"}}}}"#
            ),
        )
        .unwrap();
        fs::write(
            &director,
            format!(r#"{{"schema_version":"{DIRECTOR_SCHEMA_VERSION}"}}"#),
        )
        .unwrap();

        let mut stages = BTreeMap::new();
        for name in ["parse", "detect", "direct"] {
            stages.insert(
                name.to_string(),
                StageRecord {
                    status: StageStatus::Complete,
                    output_path: None,
                    error: None,
                },
            );
        }
        let manifest = JobManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            job_id: "d2h-test".to_string(),
            created_unix_seconds: 0,
            source: DemSource {
                path: "sample.dem".to_string(),
                byte_length: 16,
                sha256: "abc".to_string(),
                magic: "PBDEMS2".to_string(),
            },
            stages,
        };

        assert!(completed_artifacts_exist(
            &manifest,
            &timeline,
            &highlights,
            &stories,
            &director
        ));

        fs::write(&timeline, br#"{"schema_version":"1.0"}"#).unwrap();
        assert!(!completed_artifacts_exist(
            &manifest,
            &timeline,
            &highlights,
            &stories,
            &director
        ));

        fs::write(
            &timeline,
            format!(r#"{{"schema_version":"{TIMELINE_SCHEMA_VERSION}"}}"#),
        )
        .unwrap();
        fs::write(
            &highlights,
            format!(
                r#"{{"schema_version":"{HIGHLIGHT_SCHEMA_VERSION}","detector":{{"name":"{DETECTOR_NAME}","version":"old"}}}}"#
            ),
        )
        .unwrap();
        assert!(!completed_artifacts_exist(
            &manifest,
            &timeline,
            &highlights,
            &stories,
            &director
        ));

        fs::write(
            &highlights,
            format!(
                r#"{{"schema_version":"{HIGHLIGHT_SCHEMA_VERSION}","detector":{{"name":"{DETECTOR_NAME}","version":"{DETECTOR_VERSION}"}}}}"#
            ),
        )
        .unwrap();
        fs::write(&stories, br#"{"schema_version":"0.9"}"#).unwrap();
        assert!(!completed_artifacts_exist(
            &manifest,
            &timeline,
            &highlights,
            &stories,
            &director
        ));
    }
}
