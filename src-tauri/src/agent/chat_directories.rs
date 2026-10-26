//! 普通聊天分类共享目录的统一校验；保存配置与每轮运行均使用真实路径。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(crate) fn normalize_project_directories(raw: &[String]) -> Result<Vec<PathBuf>> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let app_root = super::config::resolve_home_dir()?;
    normalize_with_app_root(raw, &app_root)
}

pub(crate) fn normalize_with_app_root(raw: &[String], app_root: &Path) -> Result<Vec<PathBuf>> {
    let protected_root = match app_root.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // 尚未初始化应用根目录时仍保护它；父目录真实路径消除 HOME 的符号链接。
            let parent = app_root.parent().context("应用目录缺少父目录")?;
            parent
                .canonicalize()
                .context("无法解析应用目录的父目录")?
                .join(app_root.file_name().context("应用目录缺少名称")?)
        }
        Err(error) => return Err(error).context("无法解析应用目录的真实路径"),
    };
    let mut seen = HashSet::new();
    let mut directories = Vec::new();
    for value in raw {
        let path = Path::new(value);
        if !path.is_absolute() {
            anyhow::bail!("共享工程目录必须是绝对路径：{value}");
        }
        let canonical = path
            .canonicalize()
            .with_context(|| format!("共享工程目录不存在或无法访问：{value}"))?;
        if !canonical
            .metadata()
            .with_context(|| format!("无法读取共享工程目录：{value}"))?
            .is_dir()
        {
            anyhow::bail!("共享工程路径必须是目录：{value}");
        }
        if canonical.starts_with(&protected_root) || protected_root.starts_with(&canonical) {
            anyhow::bail!(
                "共享工程目录不能位于应用数据目录内或包含应用数据目录（{}）：{value}",
                app_root.display()
            );
        }
        if seen.insert(canonical.clone()) {
            directories.push(canonical);
        }
    }
    Ok(directories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDirGuard;

    #[test]
    fn existing_directories_are_canonicalized_and_deduplicated() {
        let dir = TempDirGuard::new("chat-directories");
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let raw = vec![
            project.display().to_string(),
            project.join(".").display().to_string(),
        ];
        let actual = normalize_with_app_root(&raw, &dir.path().join(".jkcodingagent")).unwrap();
        assert_eq!(actual, vec![project.canonicalize().unwrap()]);
    }

    #[test]
    fn rejects_relative_missing_and_file_paths() {
        let dir = TempDirGuard::new("chat-directories-invalid");
        let file = dir.path().join("document.txt");
        std::fs::write(&file, "content").unwrap();
        let app_root = dir.path().join(".jkcodingagent");
        for (path, expected) in [
            ("relative".to_string(), "绝对路径"),
            (dir.path().join("missing").display().to_string(), "不存在"),
            (file.display().to_string(), "必须是目录"),
        ] {
            let error = normalize_with_app_root(&[path], &app_root).unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    #[test]
    fn rejects_application_root_ancestors_and_descendants() {
        let dir = TempDirGuard::new("chat-directories-protected");
        let app_root = dir.path().join(".jkcodingagent");
        let error =
            normalize_with_app_root(&[dir.path().display().to_string()], &app_root).unwrap_err();
        assert!(error.to_string().contains("应用数据目录"));
        let other_session = app_root.join("sessions/other");
        std::fs::create_dir_all(&other_session).unwrap();
        for path in [dir.path(), app_root.as_path(), other_session.as_path()] {
            let error =
                normalize_with_app_root(&[path.display().to_string()], &app_root).unwrap_err();
            assert!(error.to_string().contains("应用数据目录"), "{error:#}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolves_symlinks_before_protecting_application_data() {
        let dir = TempDirGuard::new("chat-directories-symlink");
        let app_root = dir.path().join(".jkcodingagent");
        std::fs::create_dir(&app_root).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&app_root, &alias).unwrap();
        let error = normalize_with_app_root(&[alias.display().to_string()], &app_root).unwrap_err();
        assert!(error.to_string().contains("应用数据目录"));
    }
}
