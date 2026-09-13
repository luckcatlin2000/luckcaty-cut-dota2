use crate::model::{DemSource, JobManifest, MANIFEST_SCHEMA_VERSION, StageRecord, StageStatus};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const SOURCE2_MAGIC: &[u8; 8] = b"PBDEMS2\0";
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        let mut existing: JobManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
        if existing.source.sha256 != source.sha256
            || existing.source.byte_length != source.byte_length
            || existing.source.magic != source.magic
        {
            return Err(IngestError::ManifestConflict(manifest_path));
        }
        if existing.source.path != source.path {
            // The caller explicitly selected this file and inspect_dem verified its full hash.
            // Relocate the source without changing the job or any saved edit/capture state.
            existing.source.path = source.path;
            write_json_pretty(&manifest_path, &existing)?;
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
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_atomically(path, &bytes, |temporary, destination| {
        fs::rename(temporary, destination)
    })
}

fn write_atomically<F>(path: &Path, bytes: &[u8], replace: F) -> Result<(), IngestError>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let (temporary, mut file) = loop {
        let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".d2h-write-{}-{sequence}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, path)
    })();
    if result.is_err() {
        // Only the exclusive temporary file created above belongs to this operation.
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(IngestError::from)
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

    #[test]
    fn relocating_identical_content_preserves_job_and_saved_plan() {
        let temp = tempdir().unwrap();
        let original = temp.path().join("original.dem");
        let relocated = temp.path().join("relocated.dem");
        fake_dem(&original);
        let first = ingest_dem(&original, &temp.path().join("jobs")).unwrap();
        let plan = first.job_dir.join("director/edit-plan.json");
        fs::write(&plan, b"user plan must survive").unwrap();
        update_stage(
            &first.job_dir,
            "parse",
            StageStatus::Complete,
            Some("timeline/combat-events.json".into()),
            None,
        )
        .unwrap();
        fs::rename(&original, &relocated).unwrap();

        let second = ingest_dem(&relocated, &temp.path().join("jobs")).unwrap();
        assert!(second.reused_existing_job);
        assert_eq!(second.manifest.job_id, first.manifest.job_id);
        assert_eq!(
            second.manifest.created_unix_seconds,
            first.manifest.created_unix_seconds
        );
        assert_eq!(
            second.manifest.stages["parse"].status,
            StageStatus::Complete
        );
        assert_eq!(
            second.manifest.source.path,
            relocated.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(fs::read(&plan).unwrap(), b"user plan must survive");
        assert_eq!(
            ingest_dem(&relocated, &temp.path().join("jobs"))
                .unwrap()
                .manifest,
            second.manifest
        );
    }

    #[test]
    fn same_prefix_with_different_full_hash_is_still_rejected() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("replay.dem");
        fake_dem(&source);
        let first = ingest_dem(&source, &temp.path().join("jobs")).unwrap();
        let mut manifest = first.manifest;
        manifest.source.sha256.replace_range(16.., &"0".repeat(48));
        write_json_pretty(&first.job_dir.join("manifest.json"), &manifest).unwrap();
        assert!(matches!(
            ingest_dem(&source, &temp.path().join("jobs")),
            Err(IngestError::ManifestConflict(_))
        ));
    }

    #[test]
    fn atomic_json_write_replaces_complete_document_without_leaving_temp_files() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("saved.json");
        fs::write(&path, b"old document").unwrap();
        write_json_pretty(&path, &vec!["new", "complete"]).unwrap();
        assert_eq!(
            serde_json::from_slice::<Vec<String>>(&fs::read(path).unwrap()).unwrap(),
            ["new", "complete"]
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_atomic_commit_keeps_original_and_cleans_only_its_own_temp_file() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("saved.json");
        let unrelated = temp.path().join("unrelated.tmp");
        fs::write(&path, b"original").unwrap();
        fs::write(&unrelated, b"keep").unwrap();
        let result = write_atomically(&path, b"replacement", |temporary, destination| {
            assert_eq!(fs::read(temporary).unwrap(), b"replacement");
            assert_eq!(fs::read(destination).unwrap(), b"original");
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "simulated sharing violation",
            ))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(path).unwrap(), b"original");
        assert_eq!(fs::read(unrelated).unwrap(), b"keep");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }
}
