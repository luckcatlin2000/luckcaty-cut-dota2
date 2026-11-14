use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const PROTOCOL: &str = "maomao.module/1";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MODULE_CATALOG: &str = include_str!("../../module-catalog.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModuleCatalog {
    schema_version: u32,
    modules: Vec<ModuleRelease>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleRelease {
    id: String,
    name: String,
    released: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModuleManifest {
    protocol: String,
    id: String,
    tool_id: String,
    name: String,
    version: String,
    entry: String,
}

#[derive(Debug)]
struct InstalledModule {
    manifest: ModuleManifest,
    directory: PathBuf,
    executable: PathBuf,
}

#[derive(Debug)]
enum LaunchDecision {
    Unreleased,
    Missing,
    Installed(Box<InstalledModule>),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalToolLaunch {
    status: &'static str,
    tool_id: String,
    module_id: Option<String>,
    version: Option<String>,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn read_module(directory: &Path, tool_id: &str) -> Result<Option<InstalledModule>, String> {
    let manifest_path = directory.join("module.json");
    let Ok(metadata) = fs::symlink_metadata(&manifest_path) else {
        return Ok(None);
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_MANIFEST_BYTES
    {
        return Ok(None);
    }
    let manifest: ModuleManifest = match fs::read(&manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        Some(manifest) => manifest,
        None => return Ok(None),
    };
    if manifest.tool_id != tool_id {
        return Ok(None);
    }
    if manifest.protocol != PROTOCOL || !valid_id(&manifest.id) {
        return Err("独立软件接口版本不兼容，请安装支持当前接口的版本。".into());
    }
    if manifest.name.trim().is_empty() || manifest.version.trim().is_empty() {
        return Err("独立软件缺少名称或版本信息。".into());
    }
    let entry = Path::new(&manifest.entry);
    if entry.as_os_str().is_empty()
        || entry
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || !entry
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err("独立软件启动入口无效。".into());
    }
    let directory = fs::canonicalize(directory).map_err(|error| error.to_string())?;
    let executable = fs::canonicalize(directory.join(entry))
        .map_err(|_| "独立软件安装不完整，请重新安装。".to_string())?;
    if !executable.is_file() || !executable.starts_with(&directory) {
        return Err("独立软件启动入口超出安装目录。".into());
    }
    Ok(Some(InstalledModule {
        manifest,
        directory,
        executable,
    }))
}

fn discover(root: &Path, tool_id: &str) -> Result<Option<InstalledModule>, String> {
    if !valid_id(tool_id) {
        return Err("独立软件标识无效。".into());
    }
    let entries = fs::read_dir(root).map_err(|error| error.to_string())?;
    let mut found = None;
    for entry in entries.take(512).flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        if let Some(module) = read_module(&entry.path(), tool_id)? {
            if found.is_some() {
                return Err("找到多个同名独立软件，请只保留一个安装入口。".into());
            }
            found = Some(module);
        }
    }
    Ok(found)
}

/// The shipping release catalog is authoritative. A locally installed candidate
/// cannot turn an unreleased button into permission to discover or start it.
fn resolve_launch(root: &Path, tool_id: &str, catalog: &str) -> Result<LaunchDecision, String> {
    if !valid_id(tool_id) {
        return Err("独立软件标识无效。".into());
    }
    let catalog: ModuleCatalog =
        serde_json::from_str(catalog).map_err(|_| "独立软件发布目录无效，未打开任何程序。")?;
    let mut ids = std::collections::HashSet::new();
    if catalog.schema_version != 1
        || catalog.modules.len() > 64
        || catalog.modules.iter().any(|module| {
            !valid_id(&module.id)
                || module.name.trim().is_empty()
                || !ids.insert(module.id.as_str())
        })
    {
        return Err("独立软件发布目录无效，未打开任何程序。".into());
    }
    let release = catalog
        .modules
        .iter()
        .find(|module| module.id == tool_id)
        .ok_or("该独立软件尚未列入发布目录，未打开任何程序。")?;
    if !release.released {
        return Ok(LaunchDecision::Unreleased);
    }
    Ok(match discover(root, tool_id)? {
        Some(module) => LaunchDecision::Installed(Box::new(module)),
        None => LaunchDecision::Missing,
    })
}

#[tauri::command]
pub(crate) async fn open_external_tool(tool_id: String) -> Result<ExternalToolLaunch, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let root = executable.parent().ok_or("无法确定软件主目录。")?;
        let module = match resolve_launch(root, &tool_id, MODULE_CATALOG)? {
            LaunchDecision::Installed(module) => module,
            decision => {
                return Ok(ExternalToolLaunch {
                    status: match decision {
                        LaunchDecision::Unreleased => "unreleased",
                        _ => "missing",
                    },
                    tool_id,
                    module_id: None,
                    version: None,
                });
            }
        };
        let mut command = Command::new(&module.executable);
        command
            .arg("--root")
            .arg(&module.directory)
            .current_dir(&module.directory);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command
            .spawn()
            .map_err(|error| format!("无法打开独立软件：{error}"))?;
        Ok(ExternalToolLaunch {
            status: "opened",
            tool_id,
            module_id: Some(module.manifest.id),
            version: Some(module.manifest.version),
        })
    })
    .await
    .map_err(|error| format!("打开独立软件未完成：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn installed(root: &Path, folder: &str, tool: &str, entry: &str, protocol: &str) -> PathBuf {
        let directory = root.join(folder);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("module.json"), serde_json::to_vec(&serde_json::json!({
            "protocol":protocol,"id":"maomao.example","toolId":tool,"name":"Example","version":"0.1.0","entry":entry
        })).unwrap()).unwrap();
        directory
    }
    #[test]
    fn any_tool_uses_the_same_discovery_contract_without_launching() {
        let root = tempfile::tempdir().unwrap();
        let directory = installed(
            root.path(),
            "extra-tool",
            "custom-video",
            "tool.exe",
            PROTOCOL,
        );
        fs::write(directory.join("tool.exe"), b"fixture only").unwrap();
        assert!(discover(root.path(), "analysis").unwrap().is_none());
        let module = discover(root.path(), "custom-video").unwrap().unwrap();
        assert_eq!(module.manifest.version, "0.1.0");
        assert!(
            module
                .executable
                .starts_with(fs::canonicalize(directory).unwrap())
        );
    }
    #[test]
    fn traversal_and_protocol_mismatch_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        installed(root.path(), "bad", "analysis", "../outside.exe", PROTOCOL);
        assert!(discover(root.path(), "analysis").is_err());
        assert!(discover(root.path(), "../analysis").is_err());
        let other = tempfile::tempdir().unwrap();
        installed(
            other.path(),
            "wrong",
            "analysis",
            "tool.exe",
            "maomao.module/9",
        );
        assert!(discover(other.path(), "analysis").is_err());
    }
    #[test]
    fn duplicate_modules_are_not_arbitrarily_selected() {
        let root = tempfile::tempdir().unwrap();
        for name in ["one", "two"] {
            let directory = installed(root.path(), name, "analysis", "tool.exe", PROTOCOL);
            fs::write(directory.join("tool.exe"), b"fixture only").unwrap();
        }
        assert!(discover(root.path(), "analysis").is_err());
    }

    #[test]
    fn shipping_unreleased_tools_do_not_read_missing_roots_or_open_installed_candidates() {
        let root = tempfile::tempdir().unwrap();
        for tool in ["wtf", "story", "analysis"] {
            assert!(matches!(
                resolve_launch(&root.path().join("does-not-exist"), tool, MODULE_CATALOG).unwrap(),
                LaunchDecision::Unreleased
            ));
            let directory = installed(root.path(), tool, tool, "candidate.exe", PROTOCOL);
            fs::write(directory.join("candidate.exe"), b"fixture never executed").unwrap();
            assert!(matches!(
                resolve_launch(root.path(), tool, MODULE_CATALOG).unwrap(),
                LaunchDecision::Unreleased
            ));
        }
    }

    #[test]
    fn released_tools_still_require_valid_installation_and_preserve_discovery_contract() {
        let root = tempfile::tempdir().unwrap();
        let catalog =
            r#"{"schemaVersion":1,"modules":[{"id":"wtf","name":"WTF","released":true}]}"#;
        assert!(matches!(
            resolve_launch(root.path(), "wtf", catalog).unwrap(),
            LaunchDecision::Missing
        ));
        let directory = installed(root.path(), "released-wtf", "wtf", "tool.exe", PROTOCOL);
        fs::write(directory.join("tool.exe"), b"fixture never executed").unwrap();
        let LaunchDecision::Installed(module) =
            resolve_launch(root.path(), "wtf", catalog).unwrap()
        else {
            panic!("released fixture should resolve to its installed module")
        };
        assert_eq!(
            module.executable,
            fs::canonicalize(directory.join("tool.exe")).unwrap()
        );
        assert_eq!(module.manifest.tool_id, "wtf");
        installed(root.path(), "duplicate", "wtf", "tool.exe", PROTOCOL);
        fs::write(
            root.path().join("duplicate/tool.exe"),
            b"fixture never executed",
        )
        .unwrap();
        assert!(resolve_launch(root.path(), "wtf", catalog).is_err());
    }

    #[test]
    fn malformed_or_ambiguous_release_catalog_cannot_authorize_discovery() {
        let root = tempfile::tempdir().unwrap();
        for catalog in [
            r#"{"schemaVersion":99,"modules":[]}"#,
            r#"{"schemaVersion":1,"modules":[{"id":"wtf","name":"WTF","released":true},{"id":"wtf","name":"duplicate","released":false}]}"#,
            r#"{"schemaVersion":1,"modules":[{"id":"wtf","name":"WTF"}]}"#,
            "not json",
        ] {
            assert!(resolve_launch(root.path(), "wtf", catalog).is_err());
        }
        assert!(resolve_launch(root.path(), "unlisted-tool", MODULE_CATALOG).is_err());
    }
}
