use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

use crate::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplayLibrarySnapshot {
    directory: String,
    files: Vec<ReplayLibraryFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayLibraryFile {
    name: String,
    path: String,
    byte_length: u64,
    downloaded_at_unix_ms: Option<u64>,
    time_basis: ReplayTimeBasis,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReplayTimeBasis {
    Created,
    Modified,
    Unknown,
}

#[tauri::command]
pub(crate) async fn list_replays(
    directory: Option<String>,
    state: State<'_, AppState>,
) -> Result<ReplayLibrarySnapshot, String> {
    let executable = state
        .dota2_runtime
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .executable
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        let directory = resolve_directory(directory.as_deref(), executable.as_deref())?;
        scan_directory(&directory)
    })
    .await
    .map_err(|error| format!("读取录像库未完成：{error}"))?
}

fn game_directory(executable: &Path) -> Option<&Path> {
    executable.ancestors().find(|directory| {
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("game"))
    })
}

fn resolve_directory(
    directory: Option<&str>,
    executable: Option<&Path>,
) -> Result<PathBuf, String> {
    let directory = directory.unwrap_or_default().trim().trim_matches('"');
    let requested = PathBuf::from(directory);
    if requested.is_absolute() {
        return Ok(requested);
    }
    // A drive-relative or root-relative Windows path must not be silently
    // interpreted relative to whichever directory launched the application.
    if requested
        .components()
        .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
    {
        return Err("请填写完整录像目录，或相对 Dota 2 游戏目录的路径。".into());
    }
    let game = executable
        .and_then(game_directory)
        .ok_or_else(|| "请先设置 Dota 2 游戏位置，或在载入录像中选择录像目录。".to_string())?;
    if directory.is_empty() {
        return Ok(game.join("dota").join("replays"));
    }
    let first = requested
        .components()
        .find_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .unwrap_or_default();
    if first.eq_ignore_ascii_case("game") {
        return Ok(game.parent().unwrap_or(game).join(requested));
    }
    if first.eq_ignore_ascii_case("dota") {
        return Ok(game.join(requested));
    }
    Ok(game.join("dota").join(requested))
}

fn unix_millis(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
}

fn download_time(
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
) -> (Option<u64>, ReplayTimeBasis) {
    if let Some(time) = created.and_then(unix_millis) {
        (Some(time), ReplayTimeBasis::Created)
    } else if let Some(time) = modified.and_then(unix_millis) {
        (Some(time), ReplayTimeBasis::Modified)
    } else {
        (None, ReplayTimeBasis::Unknown)
    }
}

fn sort_files(files: &mut [ReplayLibraryFile]) {
    files.sort_by(|left, right| {
        right
            .downloaded_at_unix_ms
            .cmp(&left.downloaded_at_unix_ms)
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn scan_directory(directory: &Path) -> Result<ReplayLibrarySnapshot, String> {
    let directory = fs::canonicalize(directory)
        .map_err(|error| format!("无法读取录像目录 {}：{error}", directory.display()))?;
    let entries = fs::read_dir(&directory)
        .map_err(|error| format!("无法读取录像目录 {}：{error}", directory.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        // Downloads may be renamed/deleted between read_dir and metadata. Such
        // transient entries are picked up on the next refresh instead.
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dem"))
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
            continue;
        }
        let (downloaded_at_unix_ms, time_basis) =
            download_time(metadata.created().ok(), metadata.modified().ok());
        files.push(ReplayLibraryFile {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            byte_length: metadata.len(),
            downloaded_at_unix_ms,
            time_basis,
        });
    }
    sort_files(&mut files);
    Ok(ReplayLibrarySnapshot {
        directory: directory.to_string_lossy().into_owned(),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn file(name: &str, time: Option<u64>) -> ReplayLibraryFile {
        ReplayLibraryFile {
            name: name.into(),
            path: name.into(),
            byte_length: 8,
            downloaded_at_unix_ms: time,
            time_basis: ReplayTimeBasis::Created,
        }
    }

    #[test]
    fn creation_time_is_primary_with_safe_modified_and_unknown_fallbacks() {
        let early = UNIX_EPOCH + Duration::from_secs(10);
        let late = UNIX_EPOCH + Duration::from_secs(20);
        assert_eq!(
            download_time(Some(early), Some(late)),
            (Some(10_000), ReplayTimeBasis::Created)
        );
        assert_eq!(
            download_time(None, Some(late)),
            (Some(20_000), ReplayTimeBasis::Modified)
        );
        assert_eq!(
            download_time(Some(UNIX_EPOCH - Duration::from_secs(1)), Some(late)),
            (Some(20_000), ReplayTimeBasis::Modified)
        );
        assert_eq!(download_time(None, None), (None, ReplayTimeBasis::Unknown));
    }

    #[test]
    fn latest_downloads_come_first_with_stable_names_and_unknown_last() {
        let mut files = vec![
            file("unknown.dem", None),
            file("b.dem", Some(200)),
            file("old.dem", Some(100)),
            file("A.dem", Some(200)),
        ];
        sort_files(&mut files);
        assert_eq!(
            files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            vec!["A.dem", "b.dem", "old.dem", "unknown.dem"]
        );
    }

    #[test]
    fn scan_is_flat_and_only_lists_nonempty_dem_files_without_touching_contents() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("one.dem"), b"PBDEMS2\0").unwrap();
        fs::write(directory.path().join("two.DEM"), b"PBDEMS2\0").unwrap();
        fs::write(directory.path().join("downloading.dem.part"), b"partial").unwrap();
        fs::write(directory.path().join("packed.dem.bz2"), b"packed").unwrap();
        fs::write(directory.path().join("empty.dem"), b"").unwrap();
        fs::write(directory.path().join("readme.txt"), b"text").unwrap();
        fs::create_dir(directory.path().join("folder.dem")).unwrap();
        fs::write(directory.path().join("folder.dem/nested.dem"), b"nested").unwrap();
        let before = fs::read(directory.path().join("one.dem")).unwrap();
        let snapshot = scan_directory(directory.path()).unwrap();
        assert_eq!(snapshot.files.len(), 2);
        assert!(
            snapshot
                .files
                .iter()
                .all(|file| Path::new(&file.path).is_absolute())
        );
        assert_eq!(
            snapshot
                .files
                .iter()
                .map(|file| file.byte_length)
                .sum::<u64>(),
            16
        );
        assert_eq!(fs::read(directory.path().join("one.dem")).unwrap(), before);
    }

    #[test]
    fn missing_or_file_directory_returns_a_readable_error() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            scan_directory(&directory.path().join("missing"))
                .unwrap_err()
                .contains("录像目录")
        );
        let file = directory.path().join("one.dem");
        fs::write(&file, b"PBDEMS2\0").unwrap();
        assert!(scan_directory(&file).unwrap_err().contains("录像目录"));
    }

    #[test]
    fn relative_directories_are_resolved_from_the_game_not_the_process() {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game");
        let executable = game.join("bin/win64/dota2.exe");
        let expected = game.join("dota/replays");
        for requested in [
            None,
            Some(""),
            Some("replays"),
            Some("dota/replays"),
            Some("game/dota/replays"),
        ] {
            assert_eq!(
                resolve_directory(requested, Some(&executable)).unwrap(),
                expected
            );
        }
        assert_eq!(
            resolve_directory(Some("custom"), Some(&executable)).unwrap(),
            game.join("dota/custom")
        );
        assert!(resolve_directory(Some("replays"), None).is_err());
        assert_eq!(
            resolve_directory(Some(root.path().to_str().unwrap()), None).unwrap(),
            root.path()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_long_directory_paths_remain_absolute() {
        let directory = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(directory.path()).unwrap();
        assert_eq!(resolve_directory(path.to_str(), None).unwrap(), path);
        assert!(resolve_directory(Some("C:replays"), None).is_err());
        assert!(resolve_directory(Some("\\replays"), None).is_err());
    }
}
