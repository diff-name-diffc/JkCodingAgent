use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) struct Binaries {
    pub rsync: PathBuf,
    pub ssh: PathBuf,
}

/// 保留应用 PATH 顺序；桌面启动时 PATH 经常不包含 Homebrew。
fn search_dirs(path: Option<&OsStr>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = path {
        for dir in std::env::split_paths(path).filter(|dir| dir.is_absolute()) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    #[cfg(target_os = "macos")]
    for path in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        let dir = PathBuf::from(path);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn executable(path: &Path) -> Result<PathBuf, String> {
    let metadata = path.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("不是文件".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("没有执行权限".into());
        }
    }
    path.canonicalize().map_err(|e| e.to_string())
}

fn find_in(
    dirs: &[PathBuf],
    name: &str,
    validate: impl Fn(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let mut failures = Vec::new();
    let mut checked = std::collections::HashSet::new();
    for dir in dirs {
        let path = dir.join(name);
        match executable(&path) {
            Ok(resolved) => {
                if !checked.insert(resolved.clone()) {
                    continue;
                }
                match validate(&resolved) {
                    Ok(()) => return Ok(resolved),
                    Err(error) => failures.push(format!("{}：{error}", path.display())),
                }
            }
            Err(error) => failures.push(format!("{}：{error}", path.display())),
        }
    }
    let requirement = if name == "rsync" { "rsync 3.1+" } else { name };
    Err(format!("未找到可用的 {requirement}。已检查：\n{}\n请确认安装位置和执行权限；自定义安装目录需加入应用 PATH。", failures.join("\n")))
}

pub(super) fn find_binaries() -> Result<Binaries, String> {
    find_binaries_in(&search_dirs(std::env::var_os("PATH").as_deref()))
}

fn find_binaries_in(dirs: &[PathBuf]) -> Result<Binaries, String> {
    Ok(Binaries {
        rsync: find_in(dirs, "rsync", check_version)?,
        ssh: find_in(dirs, "ssh", |_| Ok(()))?,
    })
}

fn check_version(path: &Path) -> Result<(), String> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                use std::io::Read;
                let mut version = String::new();
                child
                    .stdout
                    .take()
                    .ok_or("rsync 版本输出缺失")?
                    .take(8192)
                    .read_to_string(&mut version)
                    .map_err(|e| e.to_string())?;
                if !status.success() || !supported_version(&version) {
                    return Err(format!(
                        "要求 rsync 3.1+，检测结果：{}",
                        version.lines().next().unwrap_or("无版本输出")
                    ));
                }
                return Ok(());
            }
            Ok(None) if started.elapsed() < std::time::Duration::from_secs(3) => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("rsync 版本检查失败或超时：{other:?}"));
            }
        }
    }
}

pub(super) fn supported_version(output: &str) -> bool {
    let fields: Vec<_> = output
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect();
    if fields.first() != Some(&"rsync") || fields.get(1) != Some(&"version") {
        return false;
    }
    let parts: Vec<_> = fields.get(2).unwrap_or(&"").split('.').collect();
    match (
        parts.first().and_then(|p| p.parse::<u32>().ok()),
        parts.get(1).and_then(|p| p.parse::<u32>().ok()),
    ) {
        (Some(major), Some(minor)) => major > 3 || (major == 3 && minor >= 1),
        _ => false,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("rsync-discovery-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn program(&self, directory: &str, output: &str) -> PathBuf {
            let dir = self.0.join(directory);
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join("rsync");
            std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{output}'\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            dir
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn skips_system_old_version_and_selects_later_supported_binary() {
        let f = Fixture::new();
        let old = f.program("system", "openrsync: protocol version 29");
        let new = f.program("homebrew", "rsync  version 3.5.0  protocol version 32");
        assert_eq!(
            find_in(&[old, new.clone()], "rsync", check_version).unwrap(),
            new.join("rsync").canonicalize().unwrap()
        );
    }

    #[test]
    fn preserves_first_compatible_path_and_reports_rejections() {
        let f = Fixture::new();
        let custom = f.program("custom", "rsync version 3.2.7");
        let newer = f.program("newer", "rsync version 3.5.0");
        assert_eq!(
            find_in(&[custom.clone(), newer], "rsync", check_version).unwrap(),
            custom.join("rsync").canonicalize().unwrap()
        );
        let old = f.program("old", "rsync version 2.6.9");
        let error = find_in(std::slice::from_ref(&old), "rsync", check_version).unwrap_err();
        assert!(error.contains(old.join("rsync").to_str().unwrap()));
        assert!(error.contains("2.6.9"));
    }

    #[test]
    fn ignores_relative_path_entries_and_deduplicates() {
        let paths = std::env::join_paths(["relative", "/custom/bin", "/custom/bin"]).unwrap();
        let dirs = search_dirs(Some(&paths));
        assert_eq!(dirs[0], PathBuf::from("/custom/bin"));
        assert_eq!(
            dirs.iter()
                .filter(|p| p.as_path() == std::path::Path::new("/custom/bin"))
                .count(),
            1
        );
        assert!(dirs.iter().all(|dir| dir.is_absolute()));
        #[cfg(target_os = "macos")]
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "本机集成验证，需安装 Homebrew rsync 3.1+"]
    fn installed_homebrew_rsync_is_found_with_gui_path() {
        let binaries = find_binaries_in(&search_dirs(Some(OsStr::new("/usr/bin:/bin")))).unwrap();
        check_version(&binaries.rsync).unwrap();
        assert_ne!(binaries.rsync, PathBuf::from("/usr/bin/rsync"));
    }
}
