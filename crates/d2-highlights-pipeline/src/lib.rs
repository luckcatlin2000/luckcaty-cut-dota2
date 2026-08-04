use d2_highlights_core::{
    DIRECTOR_SCHEMA_VERSION, DemSource, DirectorDocument, HIGHLIGHT_SCHEMA_VERSION,
    HighlightDocument, IngestError, JobManifest, ReplayMetadata, STORY_SCHEMA_VERSION, StageStatus,
    StoryDocument, TIMELINE_SCHEMA_VERSION, TimelineDocument, ingest_dem, update_stage,
    write_json_pretty,
};
use d2_highlights_detector::{DETECTOR_NAME, DETECTOR_VERSION, detect_highlights};
use d2_highlights_director::{StoryValidationError, build_director_plan, build_story_document};
use d2_highlights_parser_source2::{ParserAdapterError, parse_combat_timeline};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

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

    if completed_artifacts_exist(
        &ingested.manifest,
        &timeline_path,
        &highlights_path,
        &stories_path,
        &director_path,
    ) {
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

    let timeline = if stage_complete(&ingested.manifest, "parse")
        && artifact_schema_matches(&timeline_path, TIMELINE_SCHEMA_VERSION)
    {
        report(progress("parse", StageStatus::Complete, "已复用比赛事件"));
        read_json(&timeline_path)?
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
    {
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
        let highlights = detect_highlights(&timeline, 20, 18.0);
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
