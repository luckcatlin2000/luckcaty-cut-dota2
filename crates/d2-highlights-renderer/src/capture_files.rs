use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Watch the current movie prefix without reading the size of every historical frame.
/// Source 2 creates timestamp directories; unchanged old directories can be skipped.
pub(super) struct CaptureFiles {
    root: PathBuf,
    prefix: String,
    baseline: BTreeMap<PathBuf, Option<SystemTime>>,
    active: HashSet<PathBuf>,
    frame_discovery_polls: usize,
}

impl CaptureFiles {
    pub(super) fn new(root: &Path, prefix: &str) -> io::Result<Self> {
        let mut baseline = BTreeMap::new();
        snapshot_directories(root, &mut baseline)?;
        Ok(Self {
            root: root.to_path_buf(),
            prefix: prefix.to_string(),
            baseline,
            active: HashSet::new(),
            frame_discovery_polls: 0,
        })
    }

    pub(super) fn collect(&mut self) -> io::Result<BTreeMap<PathBuf, u64>> {
        // Some filesystems update directory timestamps late. A bounded discovery
        // fallback prevents an existing directory from permanently hiding a capture.
        let force_discovery = self.frame_discovery_polls >= 7;
        let mut files = BTreeMap::new();
        self.visit(&self.root.clone(), true, force_discovery, &mut files)?;
        let has_frames = files.iter().any(|(path, size)| {
            *size > 0
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"))
        });
        self.frame_discovery_polls = if !has_frames && !force_discovery {
            self.frame_discovery_polls + 1
        } else {
            0
        };
        Ok(files)
    }

    fn visit(
        &mut self,
        directory: &Path,
        root: bool,
        force_discovery: bool,
        files: &mut BTreeMap<PathBuf, u64>,
    ) -> io::Result<bool> {
        if !root && !force_discovery && !self.active.contains(directory) {
            let modified = fs::metadata(directory)?.modified().ok();
            if modified.is_some() && self.baseline.get(directory) == Some(&modified) {
                return Ok(false);
            }
        }
        let mut found = false;
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                found |= self.visit(&path, false, force_discovery, files)?;
            } else if kind.is_file()
                && entry.file_name().to_str().is_some_and(|name| {
                    name.contains(&self.prefix)
                        && (name.to_ascii_lowercase().ends_with(".jpg")
                            || name.to_ascii_lowercase().ends_with(".wav"))
                })
            {
                files.insert(path, entry.metadata()?.len());
                found = true;
            }
        }
        if found {
            // Keep polling active directories even when only file lengths change.
            self.active.insert(directory.to_path_buf());
        }
        Ok(found)
    }
}

fn snapshot_directories(
    directory: &Path,
    baseline: &mut BTreeMap<PathBuf, Option<SystemTime>>,
) -> io::Result<()> {
    baseline.insert(
        directory.to_path_buf(),
        fs::metadata(directory)?.modified().ok(),
    );
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            snapshot_directories(&entry.path(), baseline)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_capture_in_new_directory_ignores_historical_frames() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old-capture");
        fs::create_dir(&old).unwrap();
        fs::write(old.join("old-prefix0001.jpg"), b"old").unwrap();
        let mut watcher = CaptureFiles::new(root.path(), "current-prefix").unwrap();
        let current = root.path().join("new-capture");
        fs::create_dir(&current).unwrap();
        fs::write(current.join("current-prefix0001.jpg"), b"image").unwrap();
        fs::write(current.join("current-prefix.wav"), b"audio").unwrap();
        fs::write(current.join("unrelated.jpg"), b"unrelated").unwrap();
        let files = watcher.collect().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[&current.join("current-prefix0001.jpg")], 5);
        assert!(!watcher.active.contains(&old));
        fs::write(current.join("current-prefix.wav"), b"audio grows").unwrap();
        assert_eq!(
            watcher.collect().unwrap()[&current.join("current-prefix.wav")],
            11
        );
    }

    #[test]
    fn delayed_directory_timestamp_cannot_hide_a_capture() {
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("existing");
        fs::create_dir(&existing).unwrap();
        let mut watcher = CaptureFiles::new(root.path(), "new-prefix").unwrap();
        let frame = existing.join("new-prefix0001.jpg");
        fs::write(&frame, b"image").unwrap();
        // A root WAV must not disable discovery of JPEGs in a stale directory.
        fs::write(root.path().join("new-prefix.wav"), b"audio").unwrap();
        // Model a coarse filesystem timestamp: directory looks unchanged.
        watcher.baseline.insert(
            existing.clone(),
            fs::metadata(&existing).unwrap().modified().ok(),
        );
        for _ in 0..7 {
            assert!(!watcher.collect().unwrap().contains_key(&frame));
        }
        assert!(watcher.collect().unwrap().contains_key(&frame));
        assert!(watcher.active.contains(&existing));
    }

    #[test]
    fn files_at_root_and_nested_new_directories_are_discovered() {
        let root = tempfile::tempdir().unwrap();
        let mut watcher = CaptureFiles::new(root.path(), "prefix").unwrap();
        let nested = root.path().join("new").join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("prefix001.JPG"), b"a").unwrap();
        fs::write(root.path().join("prefix.wav"), b"b").unwrap();
        assert_eq!(watcher.collect().unwrap().len(), 2);
    }
}
