//! Shared final-video destinations. Recording directories and fingerprints stay unchanged.
use super::*;

const SCHEMA: &str = "d2h.video-output/1.0";
pub(super) const FILE: &str = "video-output.json";

#[derive(Clone, Default)]
pub(super) struct Settings {
    pub path: PathBuf,
    pub directory: Arc<Mutex<Option<String>>>,
}

#[derive(Serialize, Deserialize)]
struct Document {
    schema_version: String,
    directory: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    result: RenderResult,
}

impl Settings {
    pub fn load(path: PathBuf) -> Self {
        let directory = read_json::<Document>(&path)
            .ok()
            .filter(|doc| doc.schema_version == SCHEMA)
            .and_then(|doc| doc.directory);
        Self {
            path,
            directory: Arc::new(Mutex::new(directory)),
        }
    }
    pub fn current(&self) -> Option<String> {
        self.directory
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn set(&self, directory: Option<String>) -> Result<(), String> {
        let directory = validate_directory(directory)?;
        let doc = Document {
            schema_version: SCHEMA.into(),
            directory: directory.clone(),
        };
        d2_highlights_core::write_json_pretty(&self.path, &doc)
            .map_err(|e| format!("视频输出设置保存失败，原设置保留：{e}"))?;
        *self.directory.lock().unwrap_or_else(|e| e.into_inner()) = directory;
        Ok(())
    }
}

pub(super) fn validate_directory(value: Option<String>) -> Result<Option<String>, String> {
    value
        .map(|value| {
            let path = fs::canonicalize(value.trim().trim_matches('"'))
                .map_err(|e| format!("所选视频输出文件夹不存在或无法访问：{value}（{e}）"))?;
            if !path.is_dir() {
                return Err("请选择视频输出文件夹。".into());
            }
            Ok(path.display().to_string())
        })
        .transpose()
}

#[tauri::command]
pub(super) fn set_video_output_directory(
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<Capabilities, String> {
    let _lease = state.activities.begin(Activity::Save)?;
    state.video_output.set(path)?;
    Ok(capabilities_for_state(&state))
}

pub(super) fn destination(
    job: &Path,
    label: &str,
    module: &str,
    directory: Option<&str>,
) -> PathBuf {
    directory
        .map(|root| PathBuf::from(root).join(label).join(module))
        .unwrap_or_else(|| job.join("output"))
}

fn filename(title: &str) -> String {
    let value: String = title
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = value.trim_end_matches([' ', '.']);
    let stem = name.split('.').next().unwrap_or("").to_uppercase();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if name.is_empty() || reserved.contains(&stem.as_str()) {
        format!("视频_{name}")
    } else {
        name.into()
    }
}

pub(super) fn copy_new(source: &Path, directory: &Path, title: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(directory)
        .map_err(|e| format!("无法创建视频输出目录 {}：{e}", directory.display()))?;
    let name = filename(title);
    for suffix in 1..1001 {
        let target = directory.join(if suffix == 1 {
            format!("{name}.mp4")
        } else {
            format!("{name}_{suffix}.mp4")
        });
        if target.exists() {
            continue;
        }
        match fs::hard_link(source, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {}
        }
        let mut original = fs::File::open(source)
            .map_err(|e| format!("无法读取已生成成片 {}：{e}", source.display()))?;
        let mut output = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "无法写入视频 {}，原成片保留：{e}",
                    target.display()
                ));
            }
        };
        std::io::copy(&mut original, &mut output)
            .and_then(|_| output.sync_all())
            .map_err(|e| format!("视频交付未完成，原成片保留：{e}"))?;
        return Ok(target);
    }
    Err("同名成片过多，请调整输出位置。".into())
}

pub(super) fn deliver_highlight(
    state: &AppState,
    id: &str,
    directory: Option<&str>,
    mut result: RenderResult,
) -> Result<RenderResult, String> {
    let job = state.jobs_root.join(id);
    if let Some(directory) = directory {
        let timeline: TimelineDocument = read_json(&job.join("timeline/combat-events.json"))?;
        let label = timeline
            .replay
            .match_id
            .map(|n| n.to_string())
            .unwrap_or_else(|| id.into());
        let target = destination(&job, &label, "高光时刻", Some(directory));
        let title = Path::new(&result.output_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("成片名称无效。")?;
        result.output_path = copy_new(Path::new(&result.output_path), &target, title)?
            .display()
            .to_string();
    }
    d2_highlights_core::write_json_pretty(
        &job.join("render/latest-video.json"),
        &Receipt {
            result: result.clone(),
        },
    )
    .map_err(|e| format!("成片已保存，但输出记录保存失败：{e}"))?;
    Ok(result)
}

pub(super) fn latest_highlight(job: &Path) -> Option<RenderResult> {
    let receipt: Receipt = read_json(&job.join("render/latest-video.json")).ok()?;
    Path::new(&receipt.result.output_path)
        .is_file()
        .then_some(receipt.result)
}

// Allow only the video and its exact parent recorded by the native exporter.
pub(super) fn recorded_highlight_path(root: &Path, target: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .any(|entry| {
            let Some(result) = latest_highlight(&entry.path()) else {
                return false;
            };
            let Ok(video) = fs::canonicalize(result.output_path) else {
                return false;
            };
            video.extension().is_some_and(|e| e == "mp4")
                && (target == video || video.parent() == Some(target))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delivered_highlight_restores_exact_path_and_preserves_quality_and_source_assets() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState {
            jobs_root: temp.path().join("jobs"),
            dota2_runtime: Arc::new(Mutex::new(Dota2Runtime {
                executable: None,
                source: Dota2PathSource::Missing,
            })),
            dota2_settings_path: PathBuf::new(),
            video_output: Settings::default(),
            ffmpeg_exe: None,
            ffprobe_exe: None,
            render_runtime: Arc::new(Mutex::new(RenderRuntime::default())),
            activities: Activities::default(),
        };
        let id = "d2h-test";
        let job = state.jobs_root.join(id);
        d2_highlights_core::write_json_pretty(&job.join("timeline/combat-events.json"), &serde_json::json!({
            "schema_version":"1.1","source_sha256":"fixture","parser":{"name":"fixture","version":"1"},
            "replay":{"playback_ticks":1800,"playback_time_seconds":60,"game_build":42,"match_id":123,"game_mode":null,"game_winner":null,"players":[]},"events":[]
        })).unwrap();
        let source = job.join("output/123.mp4");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"completed media").unwrap();
        let result: RenderResult = serde_json::from_value(serde_json::json!({
            "outputPath":source,"qcReportPath":job.join("output/123_质量报告.json"),
            "sourceAssetsDir":job.join("output/123_源素材"),"sourceAssets":[],"durationSeconds":10,
            "width":1920,"height":1080,"segmentCount":1,"sourceAssetCount":1,"warnings":[]
        }))
        .unwrap();
        let selected = temp.path().display().to_string();
        let delivered = deliver_highlight(&state, id, Some(&selected), result.clone()).unwrap();
        assert!(Path::new(&delivered.output_path).ends_with("123/高光时刻/123.mp4"));
        assert_eq!(delivered.qc_report_path, result.qc_report_path);
        assert_eq!(delivered.source_assets_dir, result.source_assets_dir);
        assert_eq!(
            latest_highlight(&job).unwrap().output_path,
            delivered.output_path
        );
        let output = fs::canonicalize(&delivered.output_path).unwrap();
        assert!(recorded_highlight_path(&state.jobs_root, &output));
        assert!(recorded_highlight_path(
            &state.jobs_root,
            output.parent().unwrap()
        ));
        assert!(!recorded_highlight_path(&state.jobs_root, temp.path()));
        assert!(!recorded_highlight_path(
            &state.jobs_root,
            &temp.path().join("unrelated.mp4")
        ));
        let again = deliver_highlight(&state, id, Some(&selected), result.clone()).unwrap();
        assert_ne!(again.output_path, delivered.output_path);
        assert_eq!(
            fs::read(&delivered.output_path).unwrap(),
            b"completed media"
        );
        assert_eq!(fs::read(source).unwrap(), b"completed media");
        let automatic = deliver_highlight(&state, id, None, result.clone()).unwrap();
        assert_eq!(automatic.output_path, result.output_path);
    }
    #[test]
    fn shared_default_and_custom_settings_restore_without_changing_jobs() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Settings::load(temp.path().join(FILE));
        let job = temp.path().join("jobs/d2h-test");
        assert_eq!(
            destination(&job, "123", "高光时刻", None),
            destination(&job, "123", "剧情剪辑", None)
        );
        assert_eq!(
            destination(&job, "123", "高光时刻", None),
            job.join("output")
        );
        settings
            .set(Some(temp.path().display().to_string()))
            .unwrap();
        let old = settings.current();
        assert_eq!(Settings::load(settings.path.clone()).current(), old);
        assert!(
            settings
                .set(Some(temp.path().join("missing").display().to_string()))
                .is_err()
        );
        assert_eq!(settings.current(), old);
        assert_eq!(Settings::load(settings.path.clone()).current(), old);
        let source = temp.path().join("source.mp4");
        fs::write(&source, b"original video").unwrap();
        let directory = destination(&job, "123", "剧情剪辑", old.as_deref());
        let a = copy_new(&source, &directory, "河道格勒").unwrap();
        let b = copy_new(&source, &directory, "河道格勒").unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::read(a).unwrap(), b"original video");
        assert_eq!(fs::read(b).unwrap(), b"original video");
        assert_eq!(fs::read(source).unwrap(), b"original video");
        settings.set(None).unwrap();
        assert_eq!(Settings::load(settings.path.clone()).current(), None);
        assert!(!job.exists());
    }
}
