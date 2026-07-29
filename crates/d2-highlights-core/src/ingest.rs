use crate::model::{DemSource, JobManifest, MANIFEST_SCHEMA_VERSION, StageRecord, StageStatus};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const SOURCE2_MAGIC: &[u8; 8] = b"PBDEMS2\0";

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("DEM path does not exist or is not a file: {0}")]
    MissingFile(PathBuf),
    #[error("input must have a .dem extension: {0}")]
    WrongExtension(PathBuf),
    #[error("input is too small to be a Source 2 DEM: {0} bytes")]
    TooSmall(u64),
    #[error("input does not have the Source 2 PBDEMS2 header")]
    WrongMagic,
    #[error("job manifest already exists but does not match this DEM: {0}")]
    ManifestConflict(PathBuf),
    #[error("stage name is not part of the manifest: {0}")]
    UnknownStage(String),
    #[error("system clock is before the Unix epoch")]
    Clock,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Serialize)]
pub struct IngestResult {
    pub job_dir: PathBuf,
    pub manifest: JobManifest,
    pub reused_existing_job: bool,
}

pub fn inspect_dem(path: &Path) -> Result<DemSource, IngestError> {
    if !path.is_file() {
        return Err(IngestError::MissingFile(path.to_path_buf()));
    }

    let is_dem = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("dem"))
        .unwrap_or(false);
    if !is_dem {
        return Err(IngestError::WrongExtension(path.to_path_buf()));
    }

    let canonical = path.canonicalize()?;
    let file = File::open(&canonical)?;
    let byte_length = file.metadata()?.len();
    if byte_length < 16 {
        return Err(IngestError::TooSmall(byte_length));
    }

    let mut reader = BufReader::new(file);
    let mut magic = [0_u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != SOURCE2_MAGIC {
        return Err(IngestError::WrongMagic);
    }

    let mut hasher = Sha256::new();
    hasher.update(magic);
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }

    Ok(DemSource {
        path: canonical.display().to_string(),
        byte_length,
        sha256: format!("{:x}", hasher.finalize()),
        magic: "PBDEMS2".to_string(),
    })
}

pub fn ingest_dem(path: &Path, jobs_root: &Path) -> Result<IngestResult, IngestError> {
    let source = inspect_dem(path)?;
    let job_id = format!("d2h-{}", &source.sha256[..16]);
    let job_dir = jobs_root.join(&job_id);
    let manifest_path = job_dir.join("manifest.json");

    if manifest_path.is_file() {
        let existing: JobManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
        if existing.source != source {
            return Err(IngestError::ManifestConflict(manifest_path));
        }
        return Ok(IngestResult {
            job_dir,
            manifest: existing,
            reused_existing_job: true,
        });
    }

    for name in [
        "input", "timeline", "director", "capture", "edit", "qc", "logs",
    ] {
        fs::create_dir_all(job_dir.join(name))?;
    }

    let mut stages = BTreeMap::new();
    stages.insert(
        "ingest".to_string(),
        StageRecord {
            status: StageStatus::Complete,
            output_path: Some("manifest.json".to_string()),
            error: None,
        },
    );
    for name in ["parse", "detect", "direct", "capture", "edit", "qc"] {
        stages.insert(
            name.to_string(),
            StageRecord {
                status: StageStatus::Pending,
                output_path: None,
                error: None,
            },
        );
    }

    let manifest = JobManifest {
        schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
        job_id,
        created_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| IngestError::Clock)?
            .as_secs(),
        source,
        stages,
    };
    write_json_pretty(&manifest_path, &manifest)?;

    Ok(IngestResult {
        job_dir,
        manifest,
        reused_existing_job: false,
    })
}

pub fn update_stage(
    job_dir: &Path,
    stage: &str,
    status: StageStatus,
    output_path: Option<String>,
    error: Option<String>,
) -> Result<(), IngestError> {
    let manifest_path = job_dir.join("manifest.json");
    let mut manifest: JobManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    let record = manifest
        .stages
        .get_mut(stage)
        .ok_or_else(|| IngestError::UnknownStage(stage.to_string()))?;
    *record = StageRecord {
        status,
        output_path,
        error,
    };
    write_json_pretty(&manifest_path, &manifest)
}

pub fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<(), IngestError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn fake_dem(path: &Path) {
        let mut file = File::create(path).expect("create DEM fixture");
        file.write_all(SOURCE2_MAGIC).expect("write magic");
        file.write_all(&16_u32.to_le_bytes()).expect("write header");
        file.write_all(&[0_u8; 4]).expect("write header padding");
    }

    #[test]
    fn inspect_rejects_wrong_extension() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("replay.bin");
        fake_dem(&path);

        assert!(matches!(
            inspect_dem(&path),
            Err(IngestError::WrongExtension(_))
        ));
    }

    #[test]
    fn inspect_rejects_wrong_magic() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("replay.dem");
        fs::write(&path, [0_u8; 16]).unwrap();

        assert!(matches!(inspect_dem(&path), Err(IngestError::WrongMagic)));
    }

    #[test]
    fn ingest_is_idempotent_and_does_not_copy_source() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("replay.dem");
        let jobs = temp.path().join("jobs");
        fake_dem(&path);

        let first = ingest_dem(&path, &jobs).unwrap();
        let second = ingest_dem(&path, &jobs).unwrap();

        assert!(!first.reused_existing_job);
        assert!(second.reused_existing_job);
        assert_eq!(first.manifest.source, second.manifest.source);
        assert!(!first.job_dir.join("input").join("replay.dem").exists());
    }
}
