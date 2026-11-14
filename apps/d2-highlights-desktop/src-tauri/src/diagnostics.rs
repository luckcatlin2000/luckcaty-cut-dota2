//! User-requested, bounded diagnostic packages. Original task files stay in place.
use super::*;
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri_plugin_dialog::DialogExt;
use zip::write::SimpleFileOptions;

const REPORT_LIMIT: usize = 128 * 1024;
const LOG_LIMIT: usize = 4 * 1024 * 1024;
const JSON_LIMIT: usize = 1024 * 1024;
const TOTAL_LIMIT: usize = 32 * 1024 * 1024;
const FILE_LIMIT: usize = 256;
const SCAN_LIMIT: usize = 4096;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Default)]
pub(super) struct ExportedPackages(Arc<Mutex<HashSet<PathBuf>>>);

impl ExportedPackages {
    pub(super) fn allows_directory(&self, path: &Path) -> bool {
        path.is_dir() && self.0.lock().is_ok_and(|paths| paths.contains(path))
    }

    fn remember(&self, archive: &Path) {
        if let Some(directory) = archive
            .parent()
            .and_then(|path| fs::canonicalize(path).ok())
            && let Ok(mut paths) = self.0.lock()
        {
            paths.insert(directory);
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DiagnosticExportResult {
    archive_path: String,
    file_count: usize,
    warnings: Vec<String>,
}

#[tauri::command]
pub(super) async fn export_render_diagnostics(
    job_id: String,
    report: String,
    app: AppHandle,
    state: State<'_, AppState>,
    exported: State<'_, ExportedPackages>,
) -> Result<Option<DiagnosticExportResult>, String> {
    if report.len() > REPORT_LIMIT {
        return Err("排查信息过长，请使用“复制排查信息”保留原文。".into());
    }
    let job = safe_job(&state.jobs_root, &job_id)?;
    let lease = state.activities.begin(Activity::Export)?;
    let exported = exported.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        let destination = app
            .dialog()
            .file()
            .set_title("导出排查包")
            .add_filter("ZIP 排查包", &["zip"])
            .set_file_name(format!("猫猫导出排查_{job_id}_{}.zip", stamp()))
            .blocking_save_file();
        let Some(destination) = destination else {
            return Ok(None);
        };
        let destination = destination.into_path().map_err(|error| error.to_string())?;
        let result = export_to(&job, &job_id, &report, &destination)?;
        exported.remember(&destination);
        Ok(Some(result))
    })
    .await
    .map_err(|error| format!("排查包导出任务未完成：{error}"))?
}

fn stamp() -> String {
    format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn safe_job(root: &Path, job_id: &str) -> Result<PathBuf, String> {
    if !valid_job_id(job_id) {
        return Err("任务编号无效，请重新打开分析结果。".into());
    }
    let root = fs::canonicalize(root).map_err(|error| format!("任务目录无法访问：{error}"))?;
    let job = root.join(job_id);
    let metadata =
        fs::symlink_metadata(&job).map_err(|error| format!("本局任务不存在或无法访问：{error}"))?;
    if linked(&metadata) || !metadata.is_dir() {
        return Err("本局任务目录不是普通文件夹，无法导出排查包。".into());
    }
    let job = fs::canonicalize(job).map_err(|error| error.to_string())?;
    if !job.starts_with(root) {
        return Err("本局任务目录超出软件任务范围。".into());
    }
    Ok(job)
}

fn child_directory(parent: &Path, name: &str) -> Option<PathBuf> {
    let path = parent.join(name);
    if !path.exists() {
        fs::create_dir(&path).ok()?;
    }
    let metadata = fs::symlink_metadata(&path).ok()?;
    (!linked(&metadata) && metadata.is_dir()).then_some(path)
}

pub(super) struct Attempt {
    path: PathBuf,
    document: serde_json::Value,
}

/// Best effort: a diagnostic write must never replace the actual render error.
pub(super) fn begin_attempt(jobs_root: &Path, job_id: &str) -> Option<Attempt> {
    let job = safe_job(jobs_root, job_id).ok()?;
    let render = child_directory(&job, "render")?;
    let attempts = child_directory(&render, "attempts")?;
    let path = attempts.join(format!("attempt-{}.json", stamp()));
    let document = serde_json::json!({
        "schemaVersion": "d2h.render-attempt/1", "version": env!("CARGO_PKG_VERSION"),
        "jobId": job_id, "status": "started", "startedUnixMilliseconds": now_ms(),
    });
    d2_highlights_core::write_json_pretty(&path, &document).ok()?;
    Some(Attempt { path, document })
}

impl Attempt {
    pub(super) fn finish(mut self, error: Option<&str>) {
        self.document["status"] = serde_json::json!(match error {
            None => "success",
            Some(error) if error.contains("取消") => "cancelled",
            Some(_) => "failed",
        });
        self.document["finishedUnixMilliseconds"] = serde_json::json!(now_ms());
        self.document["error"] = serde_json::json!(error);
        let _ = d2_highlights_core::write_json_pretty(&self.path, &self.document);
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

struct Entry {
    name: String,
    bytes: Vec<u8>,
    truncated: bool,
    source_bytes: u64,
}

#[derive(Default)]
struct Collection {
    entries: Vec<Entry>,
    warnings: Vec<String>,
    bytes: usize,
    scanned: usize,
}

impl Collection {
    fn warn(&mut self, message: impl Into<String>) {
        if self.warnings.len() < 40 {
            self.warnings.push(message.into());
        } else if self.warnings.len() == 40 {
            self.warnings
                .push("另有更多跳过项；本包并非任务目录的完整备份。".into());
        }
    }

    fn children(&mut self, directory: &Path) -> Vec<PathBuf> {
        let Ok(metadata) = fs::symlink_metadata(directory) else {
            return Vec::new();
        };
        if linked(&metadata) || !metadata.is_dir() {
            self.warn(format!(
                "已跳过链接或非常规目录：{}",
                directory.file_name().unwrap_or_default().to_string_lossy()
            ));
            return Vec::new();
        }
        let Ok(entries) = fs::read_dir(directory) else {
            self.warn(format!(
                "无法读取诊断目录：{}",
                directory.file_name().unwrap_or_default().to_string_lossy()
            ));
            return Vec::new();
        };
        let mut result = Vec::new();
        for entry in entries {
            if self.scanned >= SCAN_LIMIT {
                self.warn("已达到诊断目录扫描上限，部分历史记录未收集。");
                break;
            }
            self.scanned += 1;
            match entry {
                Ok(entry) => result.push(entry.path()),
                Err(_) => self.warn("诊断目录中有无法读取的条目，已跳过。"),
            }
        }
        result.sort_by_key(|path| {
            std::cmp::Reverse(
                fs::symlink_metadata(path)
                    .and_then(|m| m.modified())
                    .unwrap_or(UNIX_EPOCH),
            )
        });
        result
    }

    fn file(&mut self, job: &Path, path: &Path, log: bool) {
        let Ok(relative) = path.strip_prefix(job) else {
            return;
        };
        let name = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return;
        };
        if linked(&metadata) || !metadata.is_file() {
            self.warn(format!("已跳过链接或非常规文件：{name}"));
            return;
        }
        if self.entries.len() >= FILE_LIMIT - 2 {
            self.warn("已达到排查包文件数量上限，部分历史记录未收集。");
            return;
        }
        let limit = if log { LOG_LIMIT } else { JSON_LIMIT };
        if !log && metadata.len() > limit as u64 {
            self.warn(format!("文件过大，未收集：{name}"));
            return;
        }
        let expected = metadata.len().min(limit as u64) as usize;
        if self.bytes + expected > TOTAL_LIMIT - REPORT_LIMIT - JSON_LIMIT {
            self.warn(format!("排查包达到体积上限，未收集：{name}"));
            return;
        }
        let read = (|| -> std::io::Result<Vec<u8>> {
            let mut file = fs::File::open(path)?;
            let offset = metadata.len().saturating_sub(limit as u64);
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = Vec::with_capacity(expected);
            file.take(limit as u64).read_to_end(&mut bytes)?;
            if offset > 0 {
                // A tail begins at an arbitrary byte. Discard its first partial JSONL record.
                if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                    bytes.drain(..=end);
                } else {
                    bytes.clear();
                }
            }
            Ok(bytes)
        })();
        match read {
            Ok(bytes) => {
                let truncated = metadata.len() > limit as u64;
                if truncated {
                    self.warn(format!("日志过大，仅保留末尾最多4MiB完整行：{name}"));
                }
                self.bytes += bytes.len();
                self.entries.push(Entry {
                    name,
                    bytes,
                    truncated,
                    source_bytes: metadata.len(),
                });
            }
            Err(error) => self.warn(format!("读取失败，未收集：{name}（{error}）")),
        }
    }
}

fn ordinary_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| !linked(&metadata) && metadata.is_dir())
}

fn collect(job: &Path) -> Collection {
    let mut collection = Collection::default();
    let render = job.join("render");
    if !ordinary_directory(&render) {
        collection
            .warn("未找到可读取的录制目录；已保留当前排查信息，旧任务或录制前失败可能没有日志。");
        return collection;
    }
    let logs: Vec<_> = collection
        .children(&render.join("logs"))
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("render-") && name.ends_with(".jsonl"))
        })
        .collect();
    if logs.is_empty() {
        collection.warn("未找到已保存的录制日志；包内当前排查信息仍可用于定位。");
    }
    if logs.len() > 8 {
        collection.warn("录制日志只收集最近8份，其余历史日志保留在任务目录。");
    }
    for path in logs.into_iter().take(8) {
        collection.file(job, &path, true);
    }
    let attempts: Vec<_> = collection
        .children(&render.join("attempts"))
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("attempt-") && name.ends_with(".json"))
        })
        .collect();
    if attempts.len() > 8 {
        collection.warn("导出尝试摘要只收集最近8份，其余摘要保留在任务目录。");
    }
    for path in attempts.into_iter().take(8) {
        collection.file(job, &path, false);
    }
    let caches: Vec<_> = collection
        .children(&render.join("cache"))
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.len() == 16 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        })
        .collect();
    if caches.len() > 4 {
        collection.warn("录制缓存诊断只收集最近4组，其余记录保留在任务目录。");
    }
    for cache in caches.into_iter().take(4) {
        if !ordinary_directory(&cache) {
            collection.warn("已跳过链接或非常规缓存目录。");
            continue;
        }
        collection.file(job, &cache.join("state.json"), false);
        let segments: Vec<_> = collection
            .children(&cache.join("segments"))
            .into_iter()
            .filter(|segment| {
                let Ok(metadata) = fs::symlink_metadata(segment) else {
                    return false;
                };
                if linked(&metadata) {
                    collection.warn("已跳过链接的片段目录。");
                    return false;
                }
                metadata.is_dir()
            })
            .collect();
        if segments.len() > 32 {
            collection.warn("每组缓存仅收集最近32个片段的诊断，部分片段未收集。");
        }
        for segment in segments.into_iter().take(32) {
            collection.file(job, &segment.join("capture-diagnostics.json"), false);
            collection.file(job, &segment.join("quality.json"), false);
        }
    }
    let director = job.join("director");
    if ordinary_directory(&director) {
        collection.file(job, &director.join("edit-plan.json"), false);
    }
    collection
}

fn export_to(
    job: &Path,
    job_id: &str,
    report: &str,
    target: &Path,
) -> Result<DiagnosticExportResult, String> {
    if report.len() > REPORT_LIMIT {
        return Err("排查信息超过体积上限。".into());
    }
    if !target
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        return Err("请将排查包保存为 .zip 文件。".into());
    }
    let mut collection = collect(job);
    let report = if report.trim().is_empty() {
        "当前界面无失败原文，请参阅包内历史录制记录。"
    } else {
        report
    };
    let manifest = serde_json::json!({
        "schemaVersion": "d2h.diagnostics/1", "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS, "architecture": std::env::consts::ARCH,
        "exportedUnixMilliseconds": now_ms(), "jobId": job_id,
        "includesMedia": false, "warnings": collection.warnings,
        "files": collection.entries.iter().map(|entry| serde_json::json!({
            "path": entry.name, "bytes": entry.bytes.len(), "sourceBytes": entry.source_bytes, "truncated": entry.truncated,
        })).collect::<Vec<_>>(),
    });
    let manifest = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    collection.entries.insert(
        0,
        Entry {
            name: "report.txt".into(),
            bytes: report.as_bytes().to_vec(),
            truncated: false,
            source_bytes: report.len() as u64,
        },
    );
    collection.entries.insert(
        1,
        Entry {
            name: "bundle.json".into(),
            source_bytes: manifest.len() as u64,
            bytes: manifest,
            truncated: false,
        },
    );
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "同名文件已存在，请选择新的排查包文件名。".into()
            } else {
                format!("无法创建排查包：{error}")
            }
        })?;
    let write = write_zip(file, &collection.entries);
    if let Err(error) = write {
        // Only this exclusively created, incomplete package belongs to this operation.
        let _ = fs::remove_file(target);
        return Err(format!("排查包未写完，原任务日志已保留：{error}"));
    }
    Ok(DiagnosticExportResult {
        archive_path: target.display().to_string(),
        file_count: collection.entries.len(),
        warnings: collection.warnings,
    })
}

fn write_zip(file: fs::File, entries: &[Entry]) -> Result<(), String> {
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for entry in entries {
        archive
            .start_file(&entry.name, options)
            .map_err(|error| error.to_string())?;
        archive
            .write_all(&entry.bytes)
            .map_err(|error| error.to_string())?;
    }
    archive
        .finish()
        .map_err(|error| error.to_string())?
        .sync_all()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(job: &Path, relative: &str, bytes: &[u8]) {
        let path = job.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn package(job: &Path, output: &Path) -> zip::ZipArchive<fs::File> {
        export_to(job, "d2h-example", "完整的原始故障说明", output).unwrap();
        zip::ZipArchive::new(fs::File::open(output).unwrap()).unwrap()
    }

    #[test]
    fn readable_zip_contains_only_diagnostic_whitelist_and_keeps_original_files() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("d2h-example");
        for path in [
            "render/logs/render-1.jsonl",
            "render/cache/0123456789abcdef/state.json",
            "render/cache/0123456789abcdef/segments/01-clip/capture-diagnostics.json",
            "director/edit-plan.json",
        ] {
            write(&job, path, b"not necessarily valid json");
        }
        for path in [
            "source.dem",
            "render/cache/0123456789abcdef/segments/01-clip/raw/frame.jpg",
            "render/cache/0123456789abcdef/segments/01-clip/raw/game.wav",
            "render/cache/0123456789abcdef/segments/01-clip/base.mp4",
            "render/logs/private.txt",
        ] {
            write(&job, path, b"must never be packaged");
        }
        let mut zip = package(&job, &root.path().join("diagnostics.zip"));
        assert_eq!(zip.len(), 6);
        assert!(zip.by_name("render/logs/render-1.jsonl").is_ok());
        let mut report = String::new();
        zip.by_name("report.txt")
            .unwrap()
            .read_to_string(&mut report)
            .unwrap();
        assert_eq!(report, "完整的原始故障说明");
        assert!(zip.file_names().all(|name| !name.ends_with(".mp4")
            && !name.ends_with(".dem")
            && !name.ends_with(".jpg")
            && !name.ends_with(".wav")));
        assert_eq!(
            fs::read(job.join("render/logs/render-1.jsonl")).unwrap(),
            b"not necessarily valid json"
        );
    }

    #[test]
    fn missing_logs_still_export_current_report_with_explicit_warning() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("d2h-example");
        fs::create_dir(&job).unwrap();
        let result = export_to(
            &job,
            "d2h-example",
            "failed before capture",
            &root.path().join("diagnostics.zip"),
        )
        .unwrap();
        assert_eq!(result.file_count, 2);
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("未找到"))
        );
    }

    #[test]
    fn media_beside_segment_directories_does_not_consume_diagnostic_directory_quota() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("d2h-example");
        let diagnostic = "render/cache/0123456789abcdef/segments/01-clip/capture-diagnostics.json";
        write(&job, diagnostic, b"{\"capturedFrames\":7}");
        for index in 0..40 {
            let media = format!("render/cache/0123456789abcdef/segments/{index:02}-clip.mp4");
            write(&job, &media, b"excluded media");
            fs::OpenOptions::new()
                .write(true)
                .open(job.join(media))
                .unwrap()
                .set_modified(SystemTime::now() + Duration::from_secs(10))
                .unwrap();
        }
        let mut zip = package(&job, &root.path().join("diagnostics.zip"));
        assert!(zip.by_name(diagnostic).is_ok());
        assert_eq!(zip.len(), 3);
    }

    #[test]
    fn attempts_survive_early_failure_and_interrupted_attempt_remains_started() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("d2h-example")).unwrap();
        let started = begin_attempt(root.path(), "d2h-example").unwrap();
        let started_path = started.path.clone();
        drop(started);
        let failed = begin_attempt(root.path(), "d2h-example").unwrap();
        let failed_path = failed.path.clone();
        failed.finish(Some("output directory unavailable"));
        let first: serde_json::Value =
            serde_json::from_slice(&fs::read(started_path).unwrap()).unwrap();
        let last: serde_json::Value =
            serde_json::from_slice(&fs::read(failed_path).unwrap()).unwrap();
        assert_eq!(first["status"], "started");
        assert_eq!(last["status"], "failed");
        assert_eq!(last["error"], "output directory unavailable");
        assert!(begin_attempt(root.path(), "../outside").is_none());
    }

    #[test]
    fn long_logs_export_complete_tail_records_and_manifest_discloses_truncation() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("d2h-example");
        let mut bytes = vec![b'x'; LOG_LIMIT + 20];
        bytes.extend_from_slice(b"\n{\"event\":\"last\"}\n");
        write(&job, "render/logs/render-1.jsonl", &bytes);
        let mut zip = package(&job, &root.path().join("diagnostics.zip"));
        let mut tail = String::new();
        zip.by_name("render/logs/render-1.jsonl")
            .unwrap()
            .read_to_string(&mut tail)
            .unwrap();
        assert_eq!(tail, "{\"event\":\"last\"}\n");
        let mut manifest = String::new();
        zip.by_name("bundle.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(manifest["files"][0]["truncated"], true);
        assert!(!manifest["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn existing_output_is_never_overwritten_and_non_zip_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("d2h-example");
        fs::create_dir(&job).unwrap();
        let target = root.path().join("diagnostics.zip");
        fs::write(&target, b"original").unwrap();
        assert!(export_to(&job, "d2h-example", "error", &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(export_to(&job, "d2h-example", "error", &root.path().join("file.exe")).is_err());
    }

    #[test]
    fn exported_directory_permission_is_exact_and_memory_only() {
        let root = tempfile::tempdir().unwrap();
        let other = root.path().join("other");
        fs::create_dir(&other).unwrap();
        let exported = ExportedPackages::default();
        exported.remember(&root.path().join("diagnostics.zip"));
        assert!(exported.allows_directory(&fs::canonicalize(root.path()).unwrap()));
        assert!(!exported.allows_directory(&other));
        assert!(!ExportedPackages::default().allows_directory(root.path()));
    }

    #[cfg(windows)]
    #[test]
    fn junction_directories_are_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("d2h-example");
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("render-secret.jsonl"), b"outside").unwrap();
        fs::create_dir_all(job.join("render")).unwrap();
        let junction = job.join("render").join("logs");
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        let result = export_to(
            &job,
            "d2h-example",
            "error",
            &root.path().join("diagnostics.zip"),
        )
        .unwrap();
        assert_eq!(result.file_count, 2);
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("链接"))
        );
        fs::remove_dir(&junction).unwrap();
        assert!(outside.join("render-secret.jsonl").exists());
    }
}
